//! A loopback HTTP server that feeds mpv a YouTube stream fetched in ranges.
//!
//! The guest resolver's URLs are throttled when read in one unranged GET — 0.1 MiB in 4 s,
//! measured 2026-09-23 — and that is exactly how mpv reads a URL (`Range: bytes=0-`). Asked in
//! ranges the same URL gives ~1.5 MB/s on a warm connection. So mpv is handed
//! `http://127.0.0.1:PORT/…` instead, and this answers its (ranged or not) requests by fetching
//! the upstream in ranges: a small first one so sound starts early, then 1 MiB ones, always one
//! ahead of what mpv is reading. Unlike Sonora, which downloads the whole file before decoding,
//! playback starts on the first range and memory stays at about two ranges per stream.
//!
//! Two more things fall out of sitting in the middle:
//! - **Prefetched heads.** [`StreamProxy::warm`] pulls the first 256 KiB of the *next* track
//!   while this one plays, which also leaves a pooled connection to its host open, so switching
//!   starts from memory.
//! - **Expiry without a gap.** A 403/410 from the host (the URL expired, typically after ~6 h
//!   paused) re-resolves the video and retries the same range; mpv never sees it.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use crate::{AudioStream, GUEST_USER_AGENT, VideoId};

/// The first range after a (re)start of reading: small, so the first sound waits on as little
/// as possible.
const FIRST_RANGE: u64 = 64 * 1024;
/// Every range after the first.
const RANGE: u64 = 1024 * 1024;
/// What [`StreamProxy::warm`] fetches ahead for the next track.
const HEAD: u64 = 256 * 1024;
/// Streams kept registered: the current track, the prefetched next, and a few behind for a quick
/// "previous". Older ones are forgotten; a connection still reading one keeps it alive.
const MAX_ENTRIES: usize = 6;

/// Resolve a video again, past any cache, when its URL has stopped working.
pub type Refresh =
    Arc<dyn Fn(VideoId) -> Pin<Box<dyn Future<Output = Result<AudioStream>> + Send>> + Send + Sync>;

struct Entry {
    id: VideoId,
    url: Mutex<Url>,
    length: u64,
    mime: String,
    head: Mutex<Option<Arc<Vec<u8>>>>,
}

pub struct StreamProxy {
    addr: SocketAddr,
    /// A per-run path prefix, so another local process cannot guess stream paths.
    secret: String,
    http: reqwest::Client,
    refresh: Refresh,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    order: Mutex<VecDeque<String>>,
}

impl StreamProxy {
    /// Bind to a free loopback port and start serving on `runtime`.
    pub fn start(
        runtime: &tokio::runtime::Handle,
        http: reqwest::Client,
        refresh: Refresh,
    ) -> Result<Arc<Self>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .context("could not open the stream proxy's port")?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let proxy = Arc::new(Self {
            addr,
            secret: secret(),
            http,
            refresh,
            entries: Mutex::new(HashMap::new()),
            order: Mutex::new(VecDeque::new()),
        });
        let serving = Arc::clone(&proxy);
        runtime.spawn(async move {
            let listener = match tokio::net::TcpListener::from_std(listener) {
                Ok(listener) => listener,
                Err(e) => {
                    tracing::error!(error = %e, "stream proxy: could not listen");
                    return;
                }
            };
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    continue;
                };
                let proxy = Arc::clone(&serving);
                tokio::spawn(async move {
                    if let Err(e) = proxy.serve(socket).await {
                        tracing::debug!(error = %format!("{e:#}"), "stream proxy: connection ended");
                    }
                });
            }
        });
        Ok(proxy)
    }

    /// Register `stream` for `id` and return the URL mpv should load. Re-registering a video
    /// keeps its prefetched head when the stream is the same length (the same file, re-resolved).
    pub fn url_for(&self, id: &VideoId, stream: &AudioStream) -> Result<Url> {
        let length = stream
            .content_length
            .context("a ranged stream needs a known length")?;
        let key = id.as_str().to_owned();
        let mut entries = self.entries.lock().unwrap();
        match entries.get(&key) {
            Some(entry) if entry.length == length => {
                *entry.url.lock().unwrap() = stream.url.clone();
            }
            _ => {
                entries.insert(
                    key.clone(),
                    Arc::new(Entry {
                        id: id.clone(),
                        url: Mutex::new(stream.url.clone()),
                        length,
                        mime: stream
                            .mime
                            .clone()
                            .unwrap_or_else(|| "application/octet-stream".to_owned()),
                        head: Mutex::new(None),
                    }),
                );
            }
        }
        let mut order = self.order.lock().unwrap();
        order.retain(|k| k != &key);
        order.push_back(key.clone());
        while order.len() > MAX_ENTRIES {
            if let Some(old) = order.pop_front() {
                entries.remove(&old);
            }
        }
        Url::parse(&format!("http://{}/{}/{key}", self.addr, self.secret))
            .context("could not build the proxy URL")
    }

    /// Fetch the first 256 KiB of a registered stream ahead of time. Also opens, and so warms, a
    /// pooled connection to its host.
    pub async fn warm(&self, id: &VideoId) {
        let Some(entry) = self.entries.lock().unwrap().get(id.as_str()).cloned() else {
            return;
        };
        if entry.head.lock().unwrap().is_some() {
            return;
        }
        let end = HEAD.min(entry.length) - 1;
        match self.fetch(&entry, 0, end).await {
            Ok(bytes) => *entry.head.lock().unwrap() = Some(Arc::new(bytes)),
            Err(e) => tracing::debug!(%id, error = %format!("{e:#}"), "stream proxy: warm failed"),
        }
    }

    /// Answer one request, then close: ffmpeg opens a new connection for each seek anyway.
    async fn serve(&self, mut socket: tokio::net::TcpStream) -> Result<()> {
        let request = read_request(&mut socket).await?;
        let Some(entry) = request
            .path
            .strip_prefix(&format!("/{}/", self.secret))
            .and_then(|key| self.entries.lock().unwrap().get(key).cloned())
        else {
            socket
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
            return Ok(());
        };
        let Some((start, end)) = span(request.range.as_deref(), entry.length) else {
            let header = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                entry.length
            );
            socket.write_all(header.as_bytes()).await?;
            return Ok(());
        };
        let header = if request.range.is_some() {
            format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                entry.mime,
                entry.length,
                end - start + 1
            )
        } else {
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                entry.mime, entry.length
            )
        };
        socket.write_all(header.as_bytes()).await?;
        if request.head_only {
            return Ok(());
        }
        self.stream_body(&mut socket, &entry, start, end).await
    }

    /// Write bytes `start..=end`: the prefetched head where it covers them, then upstream ranges,
    /// each fetched while the one before it is being written.
    async fn stream_body(
        &self,
        socket: &mut tokio::net::TcpStream,
        entry: &Arc<Entry>,
        start: u64,
        end: u64,
    ) -> Result<()> {
        let mut pos = start;
        let head = entry.head.lock().unwrap().clone();
        if let Some(head) = head
            && pos < head.len() as u64
        {
            let until = (end + 1).min(head.len() as u64);
            socket
                .write_all(&head[pos as usize..until as usize])
                .await?;
            pos = until;
        }
        if pos > end {
            return Ok(());
        }
        let mut next = Some(self.spawn_fetch(entry, pos, (pos + FIRST_RANGE - 1).min(end)));
        while let Some(task) = next.take() {
            let bytes = task.await.context("range task did not finish")??;
            if bytes.is_empty() {
                bail!("the stream host sent an empty range at {pos}");
            }
            pos += bytes.len() as u64;
            if pos <= end {
                next = Some(self.spawn_fetch(entry, pos, (pos + RANGE - 1).min(end)));
            }
            if let Err(e) = socket.write_all(&bytes).await {
                // mpv went away (a seek, a skip): stop fetching for it.
                if let Some(task) = next.take() {
                    task.abort();
                }
                return Err(e.into());
            }
        }
        Ok(())
    }

    fn spawn_fetch(
        &self,
        entry: &Arc<Entry>,
        start: u64,
        end: u64,
    ) -> tokio::task::JoinHandle<Result<Vec<u8>>> {
        let (http, refresh, entry) = (
            self.http.clone(),
            Arc::clone(&self.refresh),
            Arc::clone(entry),
        );
        tokio::spawn(async move { fetch_range(&http, &refresh, &entry, start, end).await })
    }

    async fn fetch(&self, entry: &Arc<Entry>, start: u64, end: u64) -> Result<Vec<u8>> {
        fetch_range(&self.http, &self.refresh, entry, start, end).await
    }
}

/// One upstream range. A refusal that means the URL has lapsed re-resolves the video and tries
/// the same range once more.
async fn fetch_range(
    http: &reqwest::Client,
    refresh: &Refresh,
    entry: &Entry,
    start: u64,
    end: u64,
) -> Result<Vec<u8>> {
    let get = |url: Url| {
        http.get(url)
            .header("Range", format!("bytes={start}-{end}"))
            .header("User-Agent", GUEST_USER_AGENT)
            .header("Accept-Encoding", "identity")
            .send()
    };
    let url = entry.url.lock().unwrap().clone();
    let mut response = get(url).await.context("cannot reach the stream host")?;
    if matches!(response.status().as_u16(), 403 | 410) {
        tracing::info!(id = %entry.id, status = %response.status(), "stream proxy: URL lapsed; resolving again");
        let fresh = refresh(entry.id.clone()).await?;
        *entry.url.lock().unwrap() = fresh.url.clone();
        response = get(fresh.url)
            .await
            .context("cannot reach the stream host")?;
    }
    let status = response.status();
    if !status.is_success() {
        bail!("the stream host answered {status} for bytes {start}-{end}");
    }
    Ok(response
        .bytes()
        .await
        .context("the stream host stopped mid-range")?
        .to_vec())
}

struct Request {
    path: String,
    range: Option<String>,
    head_only: bool,
}

/// Read a request's line and headers; mpv's requests carry no body.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Result<Request> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            bail!("the client closed before sending a request");
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 16 * 1024 {
            bail!("request headers too large");
        }
    }
    parse_request(&String::from_utf8_lossy(&buf))
}

fn parse_request(text: &str) -> Result<Request> {
    let mut lines = text.lines();
    let mut first = lines.next().context("empty request")?.split_whitespace();
    let method = first.next().context("no method")?;
    let path = first.next().context("no path")?.to_owned();
    if method != "GET" && method != "HEAD" {
        bail!("unsupported method {method}");
    }
    let range = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("range")
            .then(|| value.trim().to_owned())
    });
    Ok(Request {
        path,
        range,
        head_only: method == "HEAD",
    })
}

/// The inclusive byte span a `Range` header asks for, clamped to the file; the whole file
/// without one. `None` when it asks for nothing that exists.
fn span(range: Option<&str>, length: u64) -> Option<(u64, u64)> {
    if length == 0 {
        return None;
    }
    let last = length - 1;
    let Some(range) = range else {
        return Some((0, last));
    };
    let spec = range
        .trim()
        .strip_prefix("bytes=")?
        .split(',')
        .next()?
        .trim();
    let (from, to) = spec.split_once('-')?;
    let (start, end) = match (from.trim(), to.trim()) {
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            (length.saturating_sub(n), last)
        }
        (from, "") => (from.parse().ok()?, last),
        (from, to) => (from.parse().ok()?, to.parse::<u64>().ok()?.min(last)),
    };
    (start <= end && start <= last).then_some((start, end))
}

/// An unguessable-enough path prefix for this run, from the standard library's own random
/// hashing keys (no extra dependency for one value).
fn secret() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn ranges_are_read_the_way_ffmpeg_and_mpv_send_them() {
        assert_eq!(span(None, 1000), Some((0, 999)));
        assert_eq!(span(Some("bytes=0-"), 1000), Some((0, 999)));
        assert_eq!(span(Some("bytes=500-"), 1000), Some((500, 999)));
        assert_eq!(span(Some("bytes=10-19"), 1000), Some((10, 19)));
        assert_eq!(span(Some("bytes=990-5000"), 1000), Some((990, 999)));
        assert_eq!(span(Some("bytes=-100"), 1000), Some((900, 999)));
        assert_eq!(span(Some("bytes=1000-"), 1000), None);
        assert_eq!(span(Some("items=0-1"), 1000), None);
    }

    /// A fake stream host serving `data` in ranges, counting requests, and refusing with 403 the
    /// first `refuse` of them.
    async fn fake_host(
        data: Arc<Vec<u8>>,
        refuse: usize,
    ) -> (String, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let paths = Arc::new(Mutex::new(Vec::new()));
        let (seen, seen_paths) = (Arc::clone(&count), Arc::clone(&paths));
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let data = Arc::clone(&data);
                let n = seen.fetch_add(1, Ordering::SeqCst);
                let paths = Arc::clone(&seen_paths);
                tokio::spawn(async move {
                    let request = read_request(&mut socket).await.unwrap();
                    paths.lock().unwrap().push(request.path.clone());
                    if n < refuse {
                        let _ = socket
                            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .await;
                        return;
                    }
                    let (start, end) = span(request.range.as_deref(), data.len() as u64).unwrap();
                    let body = &data[start as usize..=end as usize];
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(body).await;
                });
            }
        });
        (base, count, paths)
    }

    fn stream(url: &str, length: usize) -> AudioStream {
        AudioStream {
            url: Url::parse(url).unwrap(),
            itag: 251,
            codec: crate::Codec::Opus,
            bitrate_kbps: 160,
            content_length: Some(length as u64),
            duration: None,
            expires_at: None,
            loudness_db: None,
            resolved_by: "guest",
            mime: Some("audio/webm".to_owned()),
            needs_ranged: true,
        }
    }

    fn data(len: usize) -> Arc<Vec<u8>> {
        Arc::new((0..len).map(|i| (i % 251) as u8).collect())
    }

    fn no_refresh() -> Refresh {
        Arc::new(|_| Box::pin(async { bail!("no refresh in this test") }))
    }

    async fn get(url: &Url, range: Option<&str>) -> (u16, Vec<u8>) {
        let mut request = reqwest::Client::new().get(url.clone());
        if let Some(range) = range {
            request = request.header("Range", range);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.bytes().await.unwrap().to_vec())
    }

    fn id() -> VideoId {
        VideoId::new("dQw4w9WgXcQ").unwrap()
    }

    /// mpv's plain request is answered in full, but upstream it is several ranged requests —
    /// never the one unranged read YouTube throttles.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_unranged_read_is_served_from_ranged_upstream_requests() {
        let body = data(2_500_000);
        let (base, count, _) = fake_host(Arc::clone(&body), 0).await;
        let proxy = StreamProxy::start(
            &tokio::runtime::Handle::current(),
            reqwest::Client::new(),
            no_refresh(),
        )
        .unwrap();
        let url = proxy
            .url_for(&id(), &stream(&format!("{base}/a"), body.len()))
            .unwrap();

        let (status, bytes) = get(&url, None).await;
        assert_eq!(status, 200);
        assert_eq!(bytes, *body);
        // 64 KiB, then 1 MiB ranges: 64K + 1M + 1M + the rest.
        assert_eq!(count.load(Ordering::SeqCst), 4);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_seek_gets_exactly_its_bytes() {
        let body = data(3_000_000);
        let (base, _, _) = fake_host(Arc::clone(&body), 0).await;
        let proxy = StreamProxy::start(
            &tokio::runtime::Handle::current(),
            reqwest::Client::new(),
            no_refresh(),
        )
        .unwrap();
        let url = proxy
            .url_for(&id(), &stream(&format!("{base}/a"), body.len()))
            .unwrap();

        let (status, bytes) = get(&url, Some("bytes=1500000-")).await;
        assert_eq!(status, 206);
        assert_eq!(bytes, body[1_500_000..]);
    }

    /// A warmed head answers the start of the stream with no request to the host at all.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_warmed_head_is_served_from_memory() {
        let body = data(1_000_000);
        let (base, count, _) = fake_host(Arc::clone(&body), 0).await;
        let proxy = StreamProxy::start(
            &tokio::runtime::Handle::current(),
            reqwest::Client::new(),
            no_refresh(),
        )
        .unwrap();
        let url = proxy
            .url_for(&id(), &stream(&format!("{base}/a"), body.len()))
            .unwrap();
        proxy.warm(&id()).await;
        let after_warm = count.load(Ordering::SeqCst);

        let (_, bytes) = get(&url, Some("bytes=0-100000")).await;
        assert_eq!(bytes, body[..=100_000]);
        assert_eq!(
            count.load(Ordering::SeqCst),
            after_warm,
            "served from the head"
        );
    }

    /// An expired URL is re-resolved and the same range fetched again; the reader just gets its
    /// bytes.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_lapsed_url_is_resolved_again_without_the_reader_noticing() {
        let body = data(200_000);
        let (base, _, paths) = fake_host(Arc::clone(&body), 1).await;
        let fresh = format!("{base}/fresh");
        let refreshed = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&refreshed);
        let len = body.len();
        let refresh: Refresh = Arc::new(move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            let fresh = fresh.clone();
            Box::pin(async move { Ok(stream(&fresh, len)) })
        });
        let proxy = StreamProxy::start(
            &tokio::runtime::Handle::current(),
            reqwest::Client::new(),
            refresh,
        )
        .unwrap();
        let url = proxy
            .url_for(&id(), &stream(&format!("{base}/stale"), len))
            .unwrap();

        let (status, bytes) = get(&url, None).await;
        assert_eq!(status, 200);
        assert_eq!(bytes, *body);
        assert_eq!(refreshed.load(Ordering::SeqCst), 1);
        let paths = paths.lock().unwrap();
        assert_eq!(paths[0], "/stale");
        assert!(paths[1..].iter().all(|p| p == "/fresh"), "{paths:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unknown_path_is_not_found() {
        let proxy = StreamProxy::start(
            &tokio::runtime::Handle::current(),
            reqwest::Client::new(),
            no_refresh(),
        )
        .unwrap();
        let url = Url::parse(&format!("http://{}/nope/dQw4w9WgXcQ", proxy.addr)).unwrap();
        assert_eq!(get(&url, None).await.0, 404);
    }
}
