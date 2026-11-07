use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail, ensure};
use moosik_spotify::{PreparedTrack, Transport};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, OnceCell},
    task::{JoinHandle, JoinSet},
};

const HTML: &str = include_str!("spotify-drm-player.html");
const LIMIT: u64 = 64 * 1024 * 1024;
const MEDIA_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const RENDERER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const PLAYBACK_READY_GRACE: Duration = Duration::from_secs(45);
const CACHE_HEADER: &[u8; 8] = b"MOOSIKC1";
static CACHE_ENABLED: AtomicBool = AtomicBool::new(true);
static CACHE_LIMIT: AtomicU64 = AtomicU64::new(512 * 1024 * 1024);
static CACHE_GENERATION: AtomicU64 = AtomicU64::new(0);
static CACHE_USAGE: AtomicU64 = AtomicU64::new(0);
static CACHE_USAGE_KNOWN: AtomicBool = AtomicBool::new(false);
static CACHE_LOCK: Mutex<()> = Mutex::new(());
static RENDERER_POOL: Mutex<RendererPool> = Mutex::new(RendererPool {
    child: None,
    ownership: RendererOwnership {
        next: 0,
        active: None,
    },
});

pub fn set_cache_enabled(enabled: bool) {
    let _guard = CACHE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if CACHE_ENABLED.swap(enabled, Ordering::SeqCst) != enabled {
        CACHE_GENERATION.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn set_cache_limit_mb(limit: u64) {
    let limit = cache_limit_bytes(limit);
    let _guard = CACHE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if CACHE_LIMIT.swap(limit, Ordering::SeqCst) != limit
        || !CACHE_USAGE_KNOWN.load(Ordering::SeqCst)
    {
        let _ = trim_cache(limit, 0);
    }
}

fn cache_limit_bytes(limit: u64) -> u64 {
    limit.clamp(
        crate::settings::SPOTIFY_CACHE_MIN_MIB,
        crate::settings::SPOTIFY_CACHE_MAX_MIB,
    ) * 1024
        * 1024
}

fn cache_entry_fits(media_len: u64, limit: u64) -> bool {
    media_len > 0
        && media_len <= LIMIT
        && media_len
            .checked_add(48)
            .is_some_and(|incoming| incoming <= limit)
}

pub fn cache_usage_bytes() -> u64 {
    if CACHE_USAGE_KNOWN.load(Ordering::SeqCst) {
        return CACHE_USAGE.load(Ordering::SeqCst);
    }
    let _guard = CACHE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let usage = cache_entries()
        .map(|entries| entries.iter().map(|entry| entry.1).sum())
        .unwrap_or(0);
    CACHE_USAGE.store(usage, Ordering::SeqCst);
    CACHE_USAGE_KNOWN.store(true, Ordering::SeqCst);
    usage
}

pub fn clear_cache() -> Result<()> {
    let _guard = CACHE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    CACHE_GENERATION.fetch_add(1, Ordering::SeqCst);
    let entries = cache_entries().context("Could not read the Spotify song cache")?;
    CACHE_USAGE_KNOWN.store(false, Ordering::SeqCst);
    for (path, _, _) in entries {
        std::fs::remove_file(path).context("Could not clear the Spotify song cache")?;
    }
    CACHE_USAGE.store(0, Ordering::SeqCst);
    CACHE_USAGE_KNOWN.store(true, Ordering::SeqCst);
    Ok(())
}

fn cache_dir() -> Result<PathBuf> {
    crate::profile_dirs()
        .map(|dirs| dirs.cache_dir().join("spotify-audio"))
        .context("Could not find Spotify song storage")
}

fn cache_name(file_id: &str) -> String {
    format!("{:x}.cenc", Sha256::digest(file_id.as_bytes()))
}

fn cache_entries() -> Result<Vec<(PathBuf, u64, SystemTime)>> {
    let entries = match std::fs::read_dir(cache_dir()?) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let complete = name.strip_suffix(".cenc");
        let partial = name.split_once(".cenc.part-").and_then(|(stem, nonce)| {
            (nonce.len() == 16 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .then_some(stem)
        });
        let Some(stem) = complete.or(partial) else {
            continue;
        };
        if stem.len() != 64 || !stem.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.is_file() && !metadata.file_type().is_symlink() {
            files.push((entry.path(), metadata.len(), metadata.modified()?));
        }
    }
    Ok(files)
}

fn trim_cache(limit: u64, incoming: u64) -> Result<()> {
    ensure!(
        incoming <= limit,
        "Spotify cache entry exceeds the cache limit"
    );
    CACHE_USAGE_KNOWN.store(false, Ordering::SeqCst);
    let mut entries = cache_entries()?;
    let mut total: u64 = entries.iter().map(|entry| entry.1).sum();
    entries.sort_by_key(|entry| entry.2);
    for (path, size, _) in entries {
        if total.saturating_add(incoming) <= limit {
            break;
        }
        std::fs::remove_file(path)?;
        total = total.saturating_sub(size);
    }
    CACHE_USAGE.store(total, Ordering::SeqCst);
    CACHE_USAGE_KNOWN.store(true, Ordering::SeqCst);
    Ok(())
}

fn decode_cache(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.get(..8)? != CACHE_HEADER {
        return None;
    }
    let len = u64::from_le_bytes(bytes.get(8..16)?.try_into().ok()?);
    let media = bytes.get(48..)?;
    if len == 0 || len > LIMIT || media.len() as u64 != len {
        return None;
    }
    (&Sha256::digest(media)[..] == bytes.get(16..48)?).then(|| media.to_vec())
}

fn read_cached(file_id: &str) -> Option<Vec<u8>> {
    let _guard = CACHE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if !CACHE_ENABLED.load(Ordering::SeqCst) {
        return None;
    }
    let path = cache_dir().ok()?.join(cache_name(file_id));
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > LIMIT + 48 {
        return None;
    }
    let mut file = std::fs::File::open(&path).ok()?;
    let mut encoded = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take(LIMIT + 49)
        .read_to_end(&mut encoded)
        .ok()?;
    let media = decode_cache(&encoded);
    if media.is_some() {
        let _ = file.set_modified(SystemTime::now());
    } else {
        drop(file);
        let _ = std::fs::remove_file(&path);
        CACHE_USAGE_KNOWN.store(false, Ordering::SeqCst);
    }
    media
}

fn write_cached(file_id: &str, bytes: &[u8], generation: u64) -> Result<()> {
    let _guard = CACHE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let limit = CACHE_LIMIT.load(Ordering::SeqCst);
    if !CACHE_ENABLED.load(Ordering::SeqCst)
        || CACHE_GENERATION.load(Ordering::SeqCst) != generation
        || !cache_entry_fits(bytes.len() as u64, limit)
    {
        return Ok(());
    }
    let directory = cache_dir()?;
    std::fs::create_dir_all(&directory)?;
    let destination = directory.join(cache_name(file_id));
    if destination.is_file() {
        return Ok(());
    }
    trim_cache(limit, bytes.len() as u64 + 48)?;
    let temporary = directory.join(format!(
        "{}.part-{:016x}",
        cache_name(file_id),
        rand::random::<u64>()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(CACHE_HEADER)?;
        file.write_all(&(bytes.len() as u64).to_le_bytes())?;
        file.write_all(&Sha256::digest(bytes))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    } else {
        CACHE_USAGE.fetch_add(bytes.len() as u64 + 48, Ordering::SeqCst);
    }
    result.map_err(Into::into)
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub idle: bool,
    pub buffering: bool,
    pub seeking: bool,
    pub rate: f64,
    pub volume: f64,
    pub muted: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            position: 0.0,
            duration: 0.0,
            paused: false,
            idle: false,
            buffering: true,
            seeking: false,
            rate: 1.0,
            volume: 70.0,
            muted: false,
        }
    }
}

#[derive(Clone)]
pub struct Handle(Arc<Inner>);

struct Inner {
    ctx: egui::Context,
    state: Mutex<Snapshot>,
    error: Mutex<Option<String>>,
    commands: Mutex<VecDeque<Value>>,
    changed: Notify,
    started: Notify,
    playing: AtomicBool,
    paused: AtomicBool,
    canceled: AtomicBool,
    resources: Mutex<Option<Resources>>,
}

struct Resources {
    server: JoinHandle<()>,
    renderer: RendererLease,
}

#[derive(Default)]
struct RendererOwnership {
    next: u64,
    active: Option<u64>,
}

impl RendererOwnership {
    fn acquire(&mut self, canceled: bool) -> Option<u64> {
        if canceled {
            return None;
        }
        self.next = self.next.wrapping_add(1);
        self.active = Some(self.next);
        self.active
    }

    fn release(&mut self, token: u64) -> bool {
        if self.active != Some(token) {
            return false;
        }
        self.active = None;
        true
    }
}

struct RendererPool {
    child: Option<Child>,
    ownership: RendererOwnership,
}

struct RendererLease {
    token: u64,
    reused: bool,
}

impl RendererLease {
    fn acquire(url: &str, handle: &Handle) -> Result<Self> {
        let mut pool = RENDERER_POOL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let token = pool
            .ownership
            .acquire(handle.0.canceled.load(Ordering::SeqCst))
            .context("Spotify playback canceled")?;
        let reused = pool.child.as_mut().is_some_and(|child| {
            child.try_wait().is_ok_and(|status| status.is_none())
                && send_renderer(
                    child,
                    crate::spotify_drm_child::RendererCommand::Load {
                        url: url.to_owned(),
                    },
                )
                .is_ok()
        });
        if !reused {
            if let Some(child) = pool.child.take() {
                retire_renderer(child);
            }
            match spawn_renderer(url) {
                Ok(child) => pool.child = Some(child),
                Err(error) => {
                    pool.ownership.release(token);
                    return Err(error);
                }
            }
        }
        Ok(Self { token, reused })
    }

    fn exited(&self) -> bool {
        let mut pool = RENDERER_POOL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        pool.ownership.active != Some(self.token)
            || pool
                .child
                .as_mut()
                .is_none_or(|child| child.try_wait().map_or(true, |status| status.is_some()))
    }
}

impl Drop for RendererLease {
    fn drop(&mut self) {
        let mut pool = RENDERER_POOL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !pool.ownership.release(self.token) {
            return;
        }
        if pool.child.as_mut().is_some_and(|child| {
            send_renderer(child, crate::spotify_drm_child::RendererCommand::Stop).is_err()
        }) && let Some(child) = pool.child.take()
        {
            retire_renderer(child);
        }
    }
}

fn send_renderer(
    child: &mut Child,
    command: crate::spotify_drm_child::RendererCommand,
) -> io::Result<()> {
    let input = child
        .stdin
        .as_mut()
        .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
    serde_json::to_writer(&mut *input, &command).map_err(io::Error::other)?;
    input.write_all(b"\n")?;
    input.flush()
}

fn spawn_renderer(url: &str) -> Result<Child> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(crate::spotify_drm_child::ARG)
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .spawn()
        .context("Could not start Spotify protected playback")
}

fn retire_renderer(mut child: Child) {
    drop(child.stdin.take());
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
    });
}

impl Drop for Resources {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.resources
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
}

impl Handle {
    pub fn new(ctx: egui::Context) -> Self {
        Self(Arc::new(Inner {
            ctx,
            state: Mutex::new(Snapshot::default()),
            error: Mutex::new(None),
            commands: Mutex::new(VecDeque::new()),
            changed: Notify::new(),
            started: Notify::new(),
            playing: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            canceled: AtomicBool::new(false),
            resources: Mutex::new(None),
        }))
    }

    pub fn state(&self) -> Snapshot {
        *self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn last_error(&self) -> Option<String> {
        self.0
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn command(&self, kind: &str, value: Value) {
        if self.0.canceled.load(Ordering::SeqCst) {
            return;
        }
        let mut commands = self.0.commands.lock().unwrap_or_else(|e| e.into_inner());
        commands.retain(|command| command["kind"] != kind);
        if commands.len() < 16 {
            commands.push_back(json!({"kind":kind,"value":value}));
        }
        drop(commands);
        self.0.changed.notify_waiters();
    }

    pub fn set_paused(&self, paused: bool) {
        if self.0.canceled.load(Ordering::SeqCst) {
            return;
        }
        self.0.paused.store(paused, Ordering::SeqCst);
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .paused = paused;
        self.command("pause", json!(paused));
    }

    pub fn seek(&self, position: f64) {
        if position.is_finite() {
            self.command("seek", json!(position.max(0.0)));
        }
    }

    pub fn set_volume(&self, volume: f64) {
        if volume.is_finite() && !self.0.canceled.load(Ordering::SeqCst) {
            let volume = volume.clamp(0.0, 100.0);
            self.0
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .volume = volume;
            self.command("volume", json!(volume));
        }
    }

    pub fn stop(&self) {
        self.0.canceled.store(true, Ordering::SeqCst);
        self.0
            .resources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.idle = true;
        state.paused = true;
        state.buffering = false;
        state.seeking = false;
        drop(state);
        self.0.changed.notify_waiters();
        self.0.started.notify_waiters();
        self.0.ctx.request_repaint();
    }

    fn fail(&self, message: String) {
        if self.0.canceled.load(Ordering::SeqCst) {
            return;
        }
        *self
            .0
            .error
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(message);
        self.stop();
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .idle = false;
    }
}

struct Broker {
    transport: Transport,
    track: PreparedTrack,
    media: OnceCell<Arc<Vec<u8>>>,
    certificate: Vec<u8>,
    prefix: String,
    origin: String,
    volume: f64,
    start_at: Option<f64>,
    cache_generation: u64,
    handle: Weak<Inner>,
}

pub async fn play(
    transport: Transport,
    uri: &str,
    handle: Handle,
    volume: f64,
    start_at: Option<f64>,
) -> Result<()> {
    let result = start(transport, uri, handle.clone(), volume, start_at).await;
    if let Err(error) = &result {
        handle.fail(format!("{error:#}"));
    }
    result
}

async fn start(
    transport: Transport,
    uri: &str,
    handle: Handle,
    volume: f64,
    start_at: Option<f64>,
) -> Result<()> {
    ensure!(
        cfg!(target_os = "windows"),
        "Spotify protected playback currently requires Windows WebView2"
    );
    ensure!(
        !handle.0.canceled.load(Ordering::SeqCst),
        "Spotify playback canceled"
    );
    let started = Instant::now();
    let cache_generation = CACHE_GENERATION.load(Ordering::SeqCst);
    let prepare = async {
        let started = Instant::now();
        let track = transport.prepare_for_playback(uri).await?;
        Result::<_>::Ok((track, started.elapsed().as_millis()))
    };
    let service_certificate = async {
        let started = Instant::now();
        let value = transport.service_certificate().await?;
        let certificate = certificate(&value)
            .context("Spotify returned an invalid service certificate")?
            .to_vec();
        Result::<_>::Ok((certificate, started.elapsed().as_millis()))
    };
    let ((track, prepare_ms), (certificate, certificate_ms)) =
        tokio::try_join!(prepare, service_certificate)?;
    ensure!(
        !handle.0.canceled.load(Ordering::SeqCst),
        "Spotify playback canceled"
    );
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let origin = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let prefix = format!("/{}/", capability());
    let url = format!("{origin}{prefix}");
    let broker = Arc::new(Broker {
        transport,
        track,
        media: OnceCell::new(),
        certificate,
        prefix,
        origin,
        volume: volume.clamp(0.0, 100.0),
        start_at,
        cache_generation,
        handle: Arc::downgrade(&handle.0),
    });
    let server_broker = broker.clone();
    let server = tokio::spawn(async move {
        let mut requests = JoinSet::new();
        let mut heartbeat = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                connection = listener.accept() => {
                    let Ok((socket, peer)) = connection else { break };
                    if !peer.ip().is_loopback() { continue; }
                    let broker = server_broker.clone();
                    requests.spawn(async move { let _ = serve(socket, broker).await; });
                }
                _ = requests.join_next(), if !requests.is_empty() => {}
                _ = heartbeat.tick() => {
                    let Some(inner) = server_broker.handle.upgrade() else { break };
                    if inner.canceled.load(Ordering::SeqCst) { break; }
                    let exited = inner.resources.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|resources| resources.renderer.exited());
                    if exited {
                        *inner.error.lock().unwrap_or_else(|e| e.into_inner()) = Some("Spotify protected renderer closed".to_owned());
                        inner.started.notify_waiters();
                        inner.ctx.request_repaint();
                        break;
                    }
                }
            }
        }
    });
    let renderer = match RendererLease::acquire(&url, &handle) {
        Ok(renderer) => renderer,
        Err(error) => {
            server.abort();
            return Err(error);
        }
    };
    let renderer_reused = renderer.reused;
    {
        let mut resources = handle.0.resources.lock().unwrap_or_else(|e| e.into_inner());
        let created = Resources { server, renderer };
        ensure!(
            !handle.0.canceled.load(Ordering::SeqCst),
            "Spotify playback canceled"
        );
        *resources = Some(created);
    }
    let renderer_started = Instant::now();
    let deadline = renderer_started
        + MEDIA_DOWNLOAD_TIMEOUT
        + RENDERER_RESPONSE_TIMEOUT
        + PLAYBACK_READY_GRACE;
    loop {
        if let Some(error) = handle.last_error() {
            bail!(error);
        }
        ensure!(
            !handle.0.canceled.load(Ordering::SeqCst),
            "Spotify playback canceled"
        );
        if handle.0.playing.load(Ordering::SeqCst) {
            tracing::info!(
                prepare_ms,
                certificate_ms,
                renderer_reused,
                renderer_ms = renderer_started.elapsed().as_millis(),
                total_ms = started.elapsed().as_millis(),
                "Spotify playback started"
            );
            return Ok(());
        }
        let exited = handle
            .0
            .resources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_none_or(|resources| resources.renderer.exited());
        ensure!(!exited, "Spotify protected renderer closed before playback");
        ensure!(
            Instant::now() < deadline,
            "Spotify protected playback timed out"
        );
        let _ = tokio::time::timeout(Duration::from_millis(100), handle.0.started.notified()).await;
    }
}

async fn serve(mut socket: TcpStream, broker: Arc<Broker>) -> Result<()> {
    let mut request = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut byte = [0u8; 1];
        while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).await?;
            request.push(byte[0]);
        }
        io::Result::Ok(())
    })
    .await??;
    ensure!(request.ends_with(b"\r\n\r\n"), "Invalid renderer request");
    let header = std::str::from_utf8(&request)?;
    let mut lines = header.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default();
    let path = first.next().unwrap_or_default();
    let mut content_len = 0usize;
    let mut origin = None;
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                content_len = value.trim().parse()?;
            }
            if key.eq_ignore_ascii_case("origin") {
                origin = Some(value.trim());
            }
        }
    }
    if !path.starts_with(&broker.prefix) || origin.is_some_and(|value| value != broker.origin) {
        return respond(&mut socket, 403, "text/plain", b"").await;
    }
    ensure!(
        content_len <= 2 * 1024 * 1024,
        "Renderer request exceeds limit"
    );
    let route = &path[broker.prefix.len()..];
    let Some(inner) = broker.handle.upgrade() else {
        return Ok(());
    };
    if inner.canceled.load(Ordering::SeqCst) {
        return Ok(());
    }
    let mut body = vec![0u8; content_len];
    if content_len > 0 {
        tokio::time::timeout(Duration::from_secs(10), socket.read_exact(&mut body)).await??;
    }
    match (method, route) {
        ("GET", "") => {
            respond(
                &mut socket,
                200,
                "text/html; charset=utf-8",
                HTML.as_bytes(),
            )
            .await
        }
        ("GET", "meta") => {
            let meta =
                serde_json::to_vec(&json!({"volume":broker.volume,"startAt":broker.start_at}))?;
            respond(&mut socket, 200, "application/json", &meta).await
        }
        ("GET", "certificate") => {
            respond(
                &mut socket,
                200,
                "application/octet-stream",
                &broker.certificate,
            )
            .await
        }
        ("GET", "media") => {
            let bytes = tokio::time::timeout(MEDIA_DOWNLOAD_TIMEOUT, async {
                let download = broker.media.get_or_try_init(|| async {
                    let started = Instant::now();
                    let file_id = broker.track.file_id.clone();
                    if let Ok(Some(bytes)) =
                        tokio::task::spawn_blocking(move || read_cached(&file_id)).await
                    {
                        tracing::info!(
                            elapsed_ms = started.elapsed().as_millis(),
                            cache_hit = true,
                            "Spotify media ready"
                        );
                        return Ok(Arc::new(bytes));
                    }
                    let first = broker
                        .transport
                        .fetch_range(&broker.track, 0, 1024 * 1024 - 1)
                        .await?;
                    ensure!(
                        first.total_len <= LIMIT && first.total_len > 0,
                        "Spotify media exceeds the 64 MiB limit"
                    );
                    let total = first.total_len;
                    let mut bytes = first.bytes;
                    ensure!(
                        bytes.len() as u64 <= total,
                        "Spotify media range exceeds its length"
                    );
                    while (bytes.len() as u64) < total {
                        ensure!(
                            !inner.canceled.load(Ordering::SeqCst),
                            "Spotify playback canceled"
                        );
                        let start = bytes.len() as u64;
                        let range = broker
                            .transport
                            .fetch_range(
                                &broker.track,
                                start,
                                (start + 1024 * 1024 - 1).min(total - 1),
                            )
                            .await?;
                        ensure!(
                            range.total_len == total
                                && !range.bytes.is_empty()
                                && start + range.bytes.len() as u64 <= total,
                            "Spotify media changed during playback"
                        );
                        bytes.extend_from_slice(&range.bytes);
                    }
                    let bytes = Arc::new(bytes);
                    if !inner.canceled.load(Ordering::SeqCst) {
                        let file_id = broker.track.file_id.clone();
                        let saved = bytes.clone();
                        let generation = broker.cache_generation;
                        drop(tokio::task::spawn_blocking(move || {
                            write_cached(&file_id, &saved, generation)
                        }));
                    }
                    tracing::info!(
                        elapsed_ms = started.elapsed().as_millis(),
                        cache_hit = false,
                        "Spotify media ready"
                    );
                    Result::<Arc<Vec<u8>>>::Ok(bytes)
                });
                download.await
            })
            .await;
            match bytes {
                Ok(Ok(bytes)) => respond(&mut socket, 200, "audio/mp4", bytes).await,
                Ok(Err(_)) | Err(_) => respond(&mut socket, 502, "text/plain", b"").await,
            }
        }
        ("POST", "license") => {
            let started = Instant::now();
            match broker.transport.license(&body).await {
                Ok(license) => {
                    tracing::info!(
                        elapsed_ms = started.elapsed().as_millis(),
                        "Spotify license ready"
                    );
                    respond(&mut socket, 200, "application/octet-stream", &license).await
                }
                Err(_) => respond(&mut socket, 502, "text/plain", b"").await,
            }
        }
        ("GET", "commands") => {
            let notified = inner.changed.notified();
            let empty = inner
                .commands
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty();
            if empty {
                let _ = tokio::time::timeout(Duration::from_secs(1), notified).await;
            }
            let commands: Vec<Value> = inner
                .commands
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain(..)
                .collect();
            respond(
                &mut socket,
                200,
                "application/json",
                &serde_json::to_vec(&commands)?,
            )
            .await
        }
        ("POST", "state") => {
            let state: BrowserState = serde_json::from_slice(&body)?;
            state.update(&inner)?;
            respond(&mut socket, 200, "text/plain", b"").await
        }
        _ => respond(&mut socket, 404, "text/plain", b"").await,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserState {
    position: f64,
    duration: f64,
    paused: bool,
    ended: bool,
    buffering: bool,
    seeking: bool,
    rate: f64,
    volume: f64,
    muted: bool,
    playing: bool,
    ready: bool,
    error_code: Option<u8>,
}

impl BrowserState {
    fn update(self, inner: &Inner) -> Result<()> {
        if inner.canceled.load(Ordering::SeqCst) {
            return Ok(());
        }
        ensure!(
            [self.position, self.duration, self.rate, self.volume]
                .iter()
                .all(|value| value.is_finite()),
            "Invalid renderer state"
        );
        ensure!(
            (0.0..=86400.0).contains(&self.position)
                && (0.0..=86400.0).contains(&self.duration)
                && (0.0..=100.0).contains(&self.volume),
            "Invalid renderer state"
        );
        if let Some(code) = self.error_code {
            let message = match code {
                1 => "Spotify playback was interrupted",
                2 => "Spotify audio could not be fetched",
                3 => "Spotify protected audio could not be decoded",
                4 => "Spotify audio format is unsupported",
                11 => "This WebView2 runtime does not support Spotify protected playback",
                12 => "This WebView2 runtime does not support Spotify audio streaming",
                _ => "Spotify protected playback failed. Sign in again and retry",
            };
            *inner.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(message.to_owned());
        }
        *inner.state.lock().unwrap_or_else(|e| e.into_inner()) = Snapshot {
            position: self.position,
            duration: self.duration,
            paused: self.paused,
            idle: self.ended && self.error_code.is_none(),
            buffering: self.buffering,
            seeking: self.seeking,
            rate: self.rate,
            volume: self.volume,
            muted: self.muted,
        };
        if self.playing || (self.ready && self.paused && inner.paused.load(Ordering::SeqCst)) {
            inner.playing.store(true, Ordering::SeqCst);
        }
        inner.started.notify_waiters();
        inner.ctx.request_repaint();
        Ok(())
    }
}

async fn respond(socket: &mut TcpStream, status: u16, mime: &str, body: &[u8]) -> Result<()> {
    let head = format!(
        "HTTP/1.1 {status} Response\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; connect-src 'self'; media-src blob:; base-uri 'none'; frame-ancestors 'none'\r\n\r\n",
        body.len()
    );
    tokio::time::timeout(RENDERER_RESPONSE_TIMEOUT, async {
        socket.write_all(head.as_bytes()).await?;
        socket.write_all(body).await
    })
    .await??;
    Ok(())
}

fn capability() -> String {
    rand::random::<[u8; 32]>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn certificate(bytes: &[u8]) -> Option<&[u8]> {
    fn varint(bytes: &[u8], at: &mut usize) -> Option<usize> {
        let mut value = 0usize;
        for shift in (0..usize::BITS).step_by(7) {
            let b = *bytes.get(*at)?;
            *at += 1;
            let group = (b & 127) as usize;
            if group > usize::MAX.checked_shr(shift)? {
                return None;
            }
            value |= group.checked_shl(shift)?;
            if b < 128 {
                return Some(value);
            }
        }
        None
    }
    let mut at = 0;
    if varint(bytes, &mut at)? != 8 || varint(bytes, &mut at)? != 5 || varint(bytes, &mut at)? != 18
    {
        return None;
    }
    let len = varint(bytes, &mut at)?;
    (at.checked_add(len)? == bytes.len()).then(|| &bytes[at..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_renderer_release_keeps_the_new_song_active() {
        let mut ownership = RendererOwnership::default();
        let old = ownership.acquire(false).unwrap();
        let current = ownership.acquire(false).unwrap();
        assert_ne!(old, current);
        assert!(!ownership.release(old));
        assert_eq!(ownership.active, Some(current));
        assert!(ownership.release(current));
        assert_eq!(ownership.active, None);
        assert!(!ownership.release(current));
    }

    #[test]
    fn canceled_renderer_acquisition_does_not_replace_the_new_song() {
        let mut ownership = RendererOwnership::default();
        let current = ownership.acquire(false).unwrap();
        assert_eq!(ownership.acquire(true), None);
        assert_eq!(ownership.active, Some(current));
        assert!(ownership.release(current));
    }

    #[test]
    fn renderer_stop_and_restart_ignore_delayed_old_releases() {
        let mut ownership = RendererOwnership::default();
        let first = ownership.acquire(false).unwrap();
        assert!(ownership.release(first));
        let second = ownership.acquire(false).unwrap();
        assert!(!ownership.release(first));
        assert_eq!(ownership.active, Some(second));
        assert!(ownership.release(second));
        let third = ownership.acquire(false).unwrap();
        assert!(!ownership.release(second));
        assert_eq!(ownership.active, Some(third));
    }

    #[test]
    fn custom_cache_capacity_uses_shared_bounds_without_the_preset_cap() {
        assert_eq!(cache_limit_bytes(4096), 4096 * 1024 * 1024);
        assert_eq!(cache_limit_bytes(1), 1024 * 1024);
        assert_eq!(
            cache_limit_bytes(0),
            crate::settings::SPOTIFY_CACHE_MIN_MIB * 1024 * 1024
        );
        assert_eq!(
            cache_limit_bytes(u64::MAX),
            crate::settings::SPOTIFY_CACHE_MAX_MIB * 1024 * 1024
        );
    }

    #[test]
    fn cache_entries_must_fit_with_their_header_before_eviction() {
        let limit = 1024 * 1024;
        assert!(cache_entry_fits(limit - 48, limit));
        assert!(!cache_entry_fits(limit - 47, limit));
        assert!(!cache_entry_fits(limit, limit));
        assert!(!cache_entry_fits(0, limit));
        assert!(!cache_entry_fits(LIMIT + 1, cache_limit_bytes(4096)));
        assert!(cache_entry_fits(LIMIT, LIMIT + 48));
    }

    #[test]
    fn stopped_playback_rejects_late_renderer_state() {
        let handle = Handle::new(egui::Context::default());
        handle.stop();
        BrowserState {
            position: 50.0,
            duration: 100.0,
            paused: false,
            ended: false,
            buffering: false,
            seeking: false,
            rate: 1.0,
            volume: 70.0,
            muted: false,
            playing: true,
            ready: true,
            error_code: None,
        }
        .update(&handle.0)
        .unwrap();
        let state = handle.state();
        assert!(state.idle && state.paused);
        assert_eq!(state.position, 0.0);
        assert!(!handle.0.playing.load(Ordering::SeqCst));
    }

    #[test]
    fn certificate_envelopes_must_match_their_declared_length() {
        assert_eq!(certificate(&[8, 5, 18, 3, 1, 2, 3]), Some(&[1, 2, 3][..]));
        assert!(certificate(&[8, 5, 18, 4, 1, 2, 3]).is_none());
        assert!(certificate(&[8, 4, 18, 3, 1, 2, 3]).is_none());
        assert!(certificate(&[8, 5, 18, 2, 1, 2, 3]).is_none());
        assert!(
            certificate(&[
                8, 5, 18, 0x83, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 2, 1, 2, 3
            ])
            .is_none()
        );
    }

    #[test]
    fn cache_entries_reject_truncation_and_changed_ciphertext() {
        let media = b"encrypted-audio-fixture";
        let mut encoded = Vec::from(CACHE_HEADER.as_slice());
        encoded.extend_from_slice(&(media.len() as u64).to_le_bytes());
        encoded.extend_from_slice(&Sha256::digest(media));
        encoded.extend_from_slice(media);
        assert_eq!(decode_cache(&encoded).as_deref(), Some(media.as_slice()));
        for length in [0, 8, 16, 47, encoded.len() - 1] {
            assert!(decode_cache(&encoded[..length]).is_none());
        }
        *encoded.last_mut().unwrap() ^= 1;
        assert!(decode_cache(&encoded).is_none());
    }

    #[test]
    fn cache_keys_keep_file_identity_out_of_paths() {
        let name = cache_name("../../../private-file");
        assert_eq!(name.len(), 69);
        assert!(name.ends_with(".cenc"));
        assert!(name[..64].bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(cache_name("file-one"), cache_name("file-two"));
    }

    #[test]
    fn stopped_handles_reject_volume_and_clear_buffering() {
        let handle = Handle::new(egui::Context::default());
        handle.stop();
        handle.set_volume(15.0);
        assert_eq!(handle.state().volume, 70.0);
        assert!(!handle.state().buffering);
        assert!(handle.0.commands.lock().unwrap().is_empty());
    }

    #[test]
    fn startup_failure_closes_playback_and_keeps_the_reason() {
        let handle = Handle::new(egui::Context::default());
        handle.fail("Unsupported protected playback".to_owned());
        assert_eq!(
            handle.last_error().as_deref(),
            Some("Unsupported protected playback")
        );
        assert!(handle.state().paused);
        assert!(!handle.state().idle);
        assert!(!handle.state().buffering);
        assert!(handle.0.canceled.load(Ordering::SeqCst));
    }
}
