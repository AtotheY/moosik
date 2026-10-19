//! Ordered fallback across several resolvers, with a circuit breaker.
//!
//! The breaker matters more than it looks. When YouTube changes the player, the native
//! resolver does not fail once, it fails for every track for the rest of the session. Without
//! a breaker each play would pay the full native timeout before falling back, making a
//! degraded app feel broken. After a few consecutive failures we stop asking until a cooldown
//! elapses, so the cost of a dead fast path is bounded.
//!
//! Two limits keep it from doing harm. A failure that belongs to one video — YouTube wanting a
//! sign-in for it, the video being gone or private — says nothing about the resolver and is not
//! counted. And the last resolver is never skipped: there is nothing after it to route to, so
//! skipping it (the whole chain, by default) only turned one bad track into ten minutes of
//! failures for every track after it, which is what the log showed on 2026-09-22 and -23.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tracing::{debug, warn};

#[cfg(feature = "native-resolver")]
use crate::NativeResolver;
use crate::{
    AudioStream, GuestResolver, StreamResolver, VideoId, YtDlpResolver, is_premium_gate,
    is_sign_in_gate,
};

/// Consecutive failures before a resolver is skipped.
const FAILURES_BEFORE_TRIP: u32 = 3;
/// How long a tripped resolver is skipped before being retried once.
const COOLDOWN: Duration = Duration::from_secs(600);

/// Whether a failure is about this one video rather than the resolver: the next video has no
/// reason to fail the same way, so the breaker does not count it.
fn is_video_specific(error: &anyhow::Error) -> bool {
    if is_sign_in_gate(error) || is_premium_gate(error) {
        return true;
    }
    let text = format!("{error:#}");
    [
        "Video unavailable",
        // The guest path's own words for the same thing.
        "This video is not available",
        "Private video",
        "This video has been removed",
    ]
    .iter()
    .any(|reason| text.contains(reason))
}

#[derive(Debug, Default)]
struct Breaker {
    consecutive_failures: u32,
    tripped_at: Option<Instant>,
}

impl Breaker {
    fn should_skip(&self) -> bool {
        match self.tripped_at {
            Some(at) => at.elapsed() < COOLDOWN,
            None => false,
        }
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.tripped_at = None;
    }

    fn record_failure(&mut self) -> bool {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= FAILURES_BEFORE_TRIP && self.tripped_at.is_none() {
            self.tripped_at = Some(Instant::now());
            return true;
        }
        // A tripped breaker whose cooldown has expired gets one more chance; if that fails
        // too, restart the cooldown rather than hammering.
        if let Some(at) = self.tripped_at
            && at.elapsed() >= COOLDOWN
        {
            self.tripped_at = Some(Instant::now());
        }
        false
    }
}

pub struct ChainResolver {
    /// Kept alongside the boxed chain so an audio-quality change can retune it without rebuilding
    /// the chain, which lives behind an `Arc` shared with in-flight resolves.
    ytdlp: std::sync::Arc<YtDlpResolver>,
    guest: Option<std::sync::Arc<GuestResolver>>,
    resolvers: Vec<Box<dyn StreamResolver>>,
    breakers: Mutex<HashMap<&'static str, Breaker>>,
}

impl ChainResolver {
    /// Resolvers are tried in the order given: fastest first, most reliable last.
    pub fn new(resolvers: Vec<Box<dyn StreamResolver>>) -> Self {
        Self {
            ytdlp: std::sync::Arc::new(YtDlpResolver::new()),
            guest: None,
            resolvers,
            breakers: Mutex::new(HashMap::new()),
        }
    }

    /// The order to use today: the guest `player` request, then yt-dlp.
    ///
    /// With the `native-resolver` feature the Boa-based resolver sits between them. It is off by
    /// default: as of the M1 gate it cannot produce playable URLs (see `native.rs`), and it costs
    /// the Boa JS engine, the largest dependency in the workspace.
    pub fn default_order(guest: GuestResolver, ytdlp: YtDlpResolver) -> Self {
        let ytdlp = std::sync::Arc::new(ytdlp);
        let guest = std::sync::Arc::new(guest);
        // The guest path first: one request, no process. yt-dlp last, so it is never skipped
        // (see the module docs) and still covers what the guest path cannot reach — a video
        // YouTube only hands to a signed-in listener, through its own signed-in retry.
        #[allow(unused_mut)]
        let mut resolvers: Vec<Box<dyn StreamResolver>> = vec![
            Box::new(std::sync::Arc::clone(&guest)),
            Box::new(std::sync::Arc::clone(&ytdlp)),
        ];
        #[cfg(feature = "native-resolver")]
        resolvers.insert(
            1,
            Box::new(NativeResolver::new(crate::NativeConfig::default())),
        );
        Self {
            ytdlp,
            guest: Some(guest),
            resolvers,
            breakers: Mutex::new(HashMap::new()),
        }
    }

    /// Retune both resolvers for the audio-quality preference, given as the yt-dlp selector.
    pub fn set_format(&self, format: &str) {
        self.ytdlp.set_format(format);
        if let Some(guest) = &self.guest {
            guest.set_quality(crate::GuestQuality::from_ytdlp_format(format));
        }
    }

    /// The intended order, for when the native resolver is healthy again.
    #[cfg(feature = "native-resolver")]
    pub fn native_first(native: NativeResolver, ytdlp: YtDlpResolver) -> Self {
        Self::new(vec![Box::new(native), Box::new(ytdlp)])
    }

    fn should_skip(&self, name: &'static str) -> bool {
        self.breakers
            .lock()
            .unwrap()
            .get(name)
            .map(Breaker::should_skip)
            .unwrap_or(false)
    }

    fn record_success(&self, name: &'static str) {
        self.breakers
            .lock()
            .unwrap()
            .entry(name)
            .or_default()
            .record_success();
    }

    fn record_failure(&self, name: &'static str) {
        let newly_tripped = {
            let mut breakers = self.breakers.lock().unwrap();
            breakers.entry(name).or_default().record_failure()
        };
        if newly_tripped {
            warn!(
                resolver = name,
                cooldown_secs = COOLDOWN.as_secs(),
                "resolver failed repeatedly; skipping it until the cooldown elapses"
            );
        }
    }
}

#[async_trait::async_trait]
impl StreamResolver for ChainResolver {
    fn name(&self) -> &'static str {
        "chain"
    }

    async fn resolve(&self, id: &VideoId) -> Result<AudioStream> {
        let mut errors = Vec::new();

        let last = self.resolvers.len().saturating_sub(1);
        for (i, resolver) in self.resolvers.iter().enumerate() {
            let name = resolver.name();
            if i < last && self.should_skip(name) {
                debug!(
                    resolver = name,
                    "skipping resolver, circuit breaker is open"
                );
                continue;
            }

            match resolver.resolve(id).await {
                Ok(stream) => {
                    self.record_success(name);
                    debug!(
                        resolver = name,
                        itag = stream.itag,
                        codec = %stream.codec,
                        bitrate_kbps = stream.bitrate_kbps,
                        "resolved"
                    );
                    return Ok(stream);
                }
                Err(e) => {
                    warn!(resolver = name, error = format!("{e:#}"), "resolver failed");
                    // The guest path is anonymous by design: once it hears Premium-only, yt-dlp
                    // can skip its own anonymous attempt and go straight to the account.
                    if is_premium_gate(&e) {
                        self.ytdlp.note_premium(id);
                    }
                    if !is_video_specific(&e) {
                        self.record_failure(name);
                    }
                    errors.push(format!("{name}: {e:#}"));
                }
            }
        }

        // Only an empty chain gets here with no errors: the last resolver is never skipped.
        if errors.is_empty() {
            bail!("every resolver is in cooldown after repeated failures; cannot resolve {id}");
        }
        bail!("all resolvers failed for {id}:\n  {}", errors.join("\n  "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Codec;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use url::Url;

    struct Stub {
        name: &'static str,
        fail: bool,
        error: &'static str,
        calls: AtomicUsize,
    }

    impl Stub {
        fn new(name: &'static str, fail: bool) -> Self {
            Self {
                name,
                fail,
                error: "stub failure",
                calls: AtomicUsize::new(0),
            }
        }

        fn failing_with(name: &'static str, error: &'static str) -> Self {
            Self {
                error,
                ..Self::new(name, true)
            }
        }
    }

    #[async_trait::async_trait]
    impl StreamResolver for std::sync::Arc<Stub> {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn resolve(&self, _id: &VideoId) -> Result<AudioStream> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                bail!("{}", self.error);
            }
            Ok(AudioStream {
                url: Url::parse("https://example.com/a").unwrap(),
                itag: 251,
                codec: Codec::Opus,
                bitrate_kbps: 160,
                content_length: None,
                duration: None,
                expires_at: None,
                loudness_db: None,
                resolved_by: "stub",
                mime: None,
                needs_ranged: false,
            })
        }
    }

    fn id() -> VideoId {
        VideoId::new("dQw4w9WgXcQ").unwrap()
    }

    #[tokio::test]
    async fn falls_through_to_the_next_resolver() {
        let bad = std::sync::Arc::new(Stub::new("bad", true));
        let good = std::sync::Arc::new(Stub::new("good", false));
        let chain = ChainResolver::new(vec![Box::new(bad.clone()), Box::new(good.clone())]);

        let s = chain.resolve(&id()).await.unwrap();
        assert_eq!(s.codec, Codec::Opus);
        assert_eq!(bad.calls.load(Ordering::SeqCst), 1);
        assert_eq!(good.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn breaker_stops_calling_a_persistently_failing_resolver() {
        let bad = std::sync::Arc::new(Stub::new("bad", true));
        let good = std::sync::Arc::new(Stub::new("good", false));
        let chain = ChainResolver::new(vec![Box::new(bad.clone()), Box::new(good.clone())]);

        for _ in 0..10 {
            chain.resolve(&id()).await.unwrap();
        }

        // Tripped after FAILURES_BEFORE_TRIP, then skipped for the rest of the run.
        assert_eq!(
            bad.calls.load(Ordering::SeqCst),
            FAILURES_BEFORE_TRIP as usize
        );
        assert_eq!(good.calls.load(Ordering::SeqCst), 10);
    }

    #[tokio::test]
    async fn success_resets_the_failure_count() {
        let flaky = std::sync::Arc::new(Stub::new("flaky", true));
        let chain = ChainResolver::new(vec![Box::new(flaky.clone())]);

        assert!(chain.resolve(&id()).await.is_err());
        chain.record_success("flaky");

        let breakers = chain.breakers.lock().unwrap();
        assert_eq!(breakers["flaky"].consecutive_failures, 0);
    }

    #[tokio::test]
    async fn reports_every_error_when_all_fail() {
        let a = std::sync::Arc::new(Stub::new("a", true));
        let b = std::sync::Arc::new(Stub::new("b", true));
        let chain = ChainResolver::new(vec![Box::new(a), Box::new(b)]);

        let err = chain.resolve(&id()).await.unwrap_err().to_string();
        assert!(err.contains("a: "), "{err}");
        assert!(err.contains("b: "), "{err}");
    }

    /// With nothing after it to route to, the last resolver is asked every time however often
    /// it has failed — skipping it would only fail every track after one bad one.
    #[tokio::test]
    async fn the_last_resolver_is_never_skipped() {
        let only = std::sync::Arc::new(Stub::new("only", true));
        let chain = ChainResolver::new(vec![Box::new(only.clone())]);

        for _ in 0..10 {
            let err = chain.resolve(&id()).await.unwrap_err().to_string();
            assert!(err.contains("only: "), "{err}");
        }
        assert_eq!(only.calls.load(Ordering::SeqCst), 10);
    }

    /// A video YouTube will not hand over anonymously says nothing about the resolver, so a run
    /// of them does not trip its breaker.
    #[tokio::test]
    async fn failures_about_one_video_do_not_trip_the_breaker() {
        let gated = std::sync::Arc::new(Stub::failing_with(
            "gated",
            "ERROR: [youtube] x: Sign in to confirm you’re not a bot.",
        ));
        let good = std::sync::Arc::new(Stub::new("good", false));
        let chain = ChainResolver::new(vec![Box::new(gated.clone()), Box::new(good)]);

        for _ in 0..10 {
            chain.resolve(&id()).await.unwrap();
        }
        assert_eq!(gated.calls.load(Ordering::SeqCst), 10);
    }

    /// The guest path's "not available" is a removed or blocked song, not a broken resolver.
    #[tokio::test]
    async fn unavailable_songs_do_not_trip_the_breaker() {
        let guest = std::sync::Arc::new(Stub::failing_with(
            "guest",
            "guest player for x: UNPLAYABLE: This video is not available",
        ));
        let good = std::sync::Arc::new(Stub::new("good", false));
        let chain = ChainResolver::new(vec![Box::new(guest.clone()), Box::new(good)]);

        for _ in 0..10 {
            chain.resolve(&id()).await.unwrap();
        }
        assert_eq!(guest.calls.load(Ordering::SeqCst), 10);
    }

    /// Premium-only refusals are about the song, so ten of them leave the breaker closed.
    #[tokio::test]
    async fn premium_refusals_do_not_trip_the_breaker() {
        let premium = std::sync::Arc::new(Stub::failing_with(
            "premium",
            "UNPLAYABLE: This video is only available to Music Premium members",
        ));
        let good = std::sync::Arc::new(Stub::new("good", false));
        let chain = ChainResolver::new(vec![Box::new(premium.clone()), Box::new(good)]);

        for _ in 0..10 {
            chain.resolve(&id()).await.unwrap();
        }
        assert_eq!(premium.calls.load(Ordering::SeqCst), 10);
    }

    /// When the guest path hears Premium-only, yt-dlp goes straight to the signed-in run: the
    /// anonymous one would only hear the same refusal.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_premium_refusal_from_the_guest_skips_yt_dlps_anonymous_run() {
        use crate::ytdlp::tests::{PREMIUM_ONLY, gated_stub, signed_in_session};

        let dir = std::env::temp_dir().join(format!("refryt-chain-premium-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (program, log) = gated_stub(&dir, PREMIUM_ONLY);
        let ytdlp = std::sync::Arc::new(
            YtDlpResolver::new()
                .with_program(program)
                .with_session(signed_in_session()),
        );
        let guest = std::sync::Arc::new(Stub::failing_with(
            "guest",
            "guest player for x: UNPLAYABLE: This video is only available to Music Premium members",
        ));
        let chain = ChainResolver {
            ytdlp: std::sync::Arc::clone(&ytdlp),
            guest: None,
            resolvers: vec![Box::new(guest), Box::new(ytdlp)],
            breakers: Mutex::new(HashMap::new()),
        };

        let stream = chain.resolve(&id()).await.unwrap();

        let runs = std::fs::read_to_string(&log).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(stream.resolved_by, "yt-dlp");
        assert_eq!(runs.lines().collect::<Vec<_>>(), ["signed"]);
    }
}
