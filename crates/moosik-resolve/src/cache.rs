//! In-memory cache of resolved URLs.
//!
//! YouTube's stream URLs carry an `expire` parameter, typically about six hours out, so
//! within a listening session a track resolved once need not be resolved again. This turns
//! replays and queue reordering into zero-latency operations and takes load off the fragile
//! resolver layer.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;

use crate::{AudioStream, StreamResolver, VideoId, VideoResolver, VideoStream};

/// Re-resolve this long before actual expiry: a URL that dies mid-track is worse than one
/// refreshed slightly early.
const FRESHNESS_MARGIN: Duration = Duration::from_secs(10 * 60);

/// One resolve per video at a time. A prefetch and a play of the same track used to race, each
/// running the full resolve — seconds of yt-dlp and hundreds of MB apiece — for one answer. The
/// second now waits its turn and finds the first one's result in the cache.
#[derive(Default)]
struct InFlight(Mutex<HashMap<VideoId, std::sync::Arc<tokio::sync::Mutex<()>>>>);

/// Held for the length of one resolve; releasing it lets the next caller for that id in.
struct Turn<'a> {
    owner: &'a InFlight,
    id: VideoId,
    lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl InFlight {
    async fn turn(&self, id: &VideoId) -> Turn<'_> {
        let lock = std::sync::Arc::clone(self.0.lock().unwrap().entry(id.clone()).or_default());
        let guard = std::sync::Arc::clone(&lock).lock_owned().await;
        Turn {
            owner: self,
            id: id.clone(),
            lock,
            _guard: guard,
        }
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut map = self.owner.0.lock().unwrap();
        // The map's own reference, this turn's, and the guard's: nobody else is waiting.
        if std::sync::Arc::strong_count(&self.lock) <= 3 {
            map.remove(&self.id);
        }
    }
}

pub struct StreamCache<R> {
    inner: R,
    entries: Mutex<HashMap<VideoId, AudioStream>>,
    in_flight: InFlight,
}

impl<R: StreamResolver> StreamCache<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            entries: Mutex::new(HashMap::new()),
            in_flight: InFlight::default(),
        }
    }

    /// The resolver underneath, so callers can retune it.
    pub fn inner(&self) -> &R {
        &self.inner
    }

    /// Forget every cached URL. Needed when the *kind* of stream we want changes — a cached
    /// high-quality URL would otherwise keep playing after switching to a lower one.
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }

    pub fn get_cached(&self, id: &VideoId) -> Option<AudioStream> {
        let entries = self.entries.lock().unwrap();
        entries
            .get(id)
            .filter(|s| s.is_fresh(FRESHNESS_MARGIN))
            .cloned()
    }

    /// Drop expired entries. Cheap; call when convenient.
    /// Resolve `id` again, past its cached entry — for a URL the host has stopped honouring.
    pub async fn refresh(&self, id: &VideoId) -> Result<AudioStream> {
        self.entries.lock().unwrap().remove(id);
        self.resolve(id).await
    }

    pub fn evict_stale(&self) {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, s| s.is_fresh(FRESHNESS_MARGIN));
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait::async_trait]
impl<R: StreamResolver> StreamResolver for StreamCache<R> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn resolve(&self, id: &VideoId) -> Result<AudioStream> {
        if let Some(hit) = self.get_cached(id) {
            return Ok(hit);
        }
        let _turn = self.in_flight.turn(id).await;
        // Whoever held the turn before may have just filled the cache.
        if let Some(hit) = self.get_cached(id) {
            return Ok(hit);
        }
        let stream = self.inner.resolve(id).await?;
        self.entries
            .lock()
            .unwrap()
            .insert(id.clone(), stream.clone());
        Ok(stream)
    }
}

/// The same cache for [`VideoStream`]s, kept separate because a video URL and an audio URL for
/// the same id are different answers to different questions — and the video path is prefetched
/// while the audio one may be playing.
pub struct VideoStreamCache<R> {
    inner: R,
    entries: Mutex<HashMap<VideoId, VideoStream>>,
    in_flight: InFlight,
}

impl<R: VideoResolver> VideoStreamCache<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            entries: Mutex::new(HashMap::new()),
            in_flight: InFlight::default(),
        }
    }

    /// The resolver underneath, so callers can retune it.
    pub fn inner(&self) -> &R {
        &self.inner
    }

    pub fn get_cached(&self, id: &VideoId) -> Option<VideoStream> {
        let entries = self.entries.lock().unwrap();
        entries
            .get(id)
            .filter(|s| s.is_fresh(FRESHNESS_MARGIN))
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait::async_trait]
impl<R: VideoResolver> VideoResolver for VideoStreamCache<R> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn resolve_video(&self, id: &VideoId) -> Result<VideoStream> {
        if let Some(hit) = self.get_cached(id) {
            return Ok(hit);
        }
        let _turn = self.in_flight.turn(id).await;
        if let Some(hit) = self.get_cached(id) {
            return Ok(hit);
        }
        let stream = self.inner.resolve_video(id).await?;
        self.entries
            .lock()
            .unwrap()
            .insert(id.clone(), stream.clone());
        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Codec;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;
    use url::Url;

    struct Counting {
        calls: AtomicUsize,
        expires_in: Option<Duration>,
    }

    #[async_trait::async_trait]
    impl StreamResolver for std::sync::Arc<Counting> {
        fn name(&self) -> &'static str {
            "counting"
        }
        async fn resolve(&self, _id: &VideoId) -> Result<AudioStream> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // Long enough for a second caller to arrive while this one is still resolving.
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok(AudioStream {
                url: Url::parse("https://example.com/a").unwrap(),
                itag: 251,
                codec: Codec::Opus,
                bitrate_kbps: 160,
                content_length: None,
                duration: None,
                expires_at: self.expires_in.map(|d| SystemTime::now() + d),
                loudness_db: None,
                resolved_by: "counting",
                mime: None,
                needs_ranged: false,
            })
        }
    }

    fn id() -> VideoId {
        VideoId::new("dQw4w9WgXcQ").unwrap()
    }

    #[tokio::test]
    async fn second_resolve_is_served_from_cache() {
        let inner = std::sync::Arc::new(Counting {
            calls: AtomicUsize::new(0),
            expires_in: Some(Duration::from_secs(6 * 3600)),
        });
        let cache = StreamCache::new(inner.clone());

        cache.resolve(&id()).await.unwrap();
        cache.resolve(&id()).await.unwrap();

        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_url_near_expiry_is_re_resolved() {
        let inner = std::sync::Arc::new(Counting {
            calls: AtomicUsize::new(0),
            // Inside the freshness margin, so it must not be reused.
            expires_in: Some(Duration::from_secs(60)),
        });
        let cache = StreamCache::new(inner.clone());

        cache.resolve(&id()).await.unwrap();
        cache.resolve(&id()).await.unwrap();

        assert_eq!(inner.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn evict_stale_drops_expired_entries() {
        let inner = std::sync::Arc::new(Counting {
            calls: AtomicUsize::new(0),
            expires_in: Some(Duration::from_secs(60)),
        });
        let cache = StreamCache::new(inner);

        cache.resolve(&id()).await.unwrap();
        assert_eq!(cache.len(), 1);
        cache.evict_stale();
        assert!(cache.is_empty());
    }

    struct CountingVideo {
        calls: AtomicUsize,
        expires_in: Option<Duration>,
    }

    #[async_trait::async_trait]
    impl VideoResolver for std::sync::Arc<CountingVideo> {
        fn name(&self) -> &'static str {
            "counting-video"
        }
        async fn resolve_video(&self, _id: &VideoId) -> Result<VideoStream> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(VideoStream {
                video: Url::parse("https://example.com/v").unwrap(),
                audio: Some(Url::parse("https://example.com/a").unwrap()),
                duration: None,
                expires_at: self.expires_in.map(|d| SystemTime::now() + d),
                resolved_by: "counting-video",
            })
        }
    }

    #[tokio::test]
    async fn a_video_resolve_is_served_from_cache_the_second_time() {
        let inner = std::sync::Arc::new(CountingVideo {
            calls: AtomicUsize::new(0),
            expires_in: Some(Duration::from_secs(6 * 3600)),
        });
        let cache = VideoStreamCache::new(inner.clone());

        let first = cache.resolve_video(&id()).await.unwrap();
        let second = cache.resolve_video(&id()).await.unwrap();

        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.audio, second.audio);
    }

    #[tokio::test]
    async fn a_video_url_near_expiry_is_re_resolved() {
        let inner = std::sync::Arc::new(CountingVideo {
            calls: AtomicUsize::new(0),
            expires_in: Some(Duration::from_secs(60)),
        });
        let cache = VideoStreamCache::new(inner.clone());

        cache.resolve_video(&id()).await.unwrap();
        cache.resolve_video(&id()).await.unwrap();

        assert_eq!(inner.calls.load(Ordering::SeqCst), 2);
    }

    /// A prefetch and a play of the same track at once: one resolve, both answered.
    #[tokio::test]
    async fn concurrent_resolves_of_one_video_share_the_work() {
        let inner = std::sync::Arc::new(Counting {
            calls: AtomicUsize::new(0),
            expires_in: Some(Duration::from_secs(6 * 3600)),
        });
        let cache = StreamCache::new(inner.clone());

        let video = id();
        let (a, b) = tokio::join!(cache.resolve(&video), cache.resolve(&video));
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        assert!(
            cache.in_flight.0.lock().unwrap().is_empty(),
            "turns are cleaned up"
        );
    }
}
