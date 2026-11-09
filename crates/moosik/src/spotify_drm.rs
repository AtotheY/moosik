use std::{
    collections::VecDeque,
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail, ensure};
use moosik_spotify::{
    PlaybackReport, PlaybackReportKind, PlaybackTimeline, PreparedTrack, RangeData, Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{Mutex as AsyncMutex, Notify, mpsc, watch},
    task::{JoinHandle, JoinSet},
};

const HTML: &str = include_str!("spotify-drm-player.html");
const LIMIT: u64 = 64 * 1024 * 1024;
const FIRST_MEDIA_RANGE: u64 = 128 * 1024;
const MEDIA_RANGE: u64 = 1024 * 1024;
const MEDIA_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const RENDERER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const RENDERER_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
const PLAYBACK_READY_GRACE: Duration = Duration::from_secs(45);
const REPORT_STARTED_MS: u64 = 1_000;
const REPORT_THRESHOLD_MS: u64 = 31_000;
const REPORT_AUTH_TIMEOUT: Duration = Duration::from_secs(2);
const LEGACY_CACHE_HEADER: &[u8; 8] = b"MOOSIKC1";
const CACHE_HEADER: &[u8; 8] = b"MOOSIKC2";
const CACHE_HEADER_LEN: u64 = 80;
static CACHE_ENABLED: AtomicBool = AtomicBool::new(true);
static CACHE_LIMIT: AtomicU64 = AtomicU64::new(512 * 1024 * 1024);
static CACHE_GENERATION: AtomicU64 = AtomicU64::new(0);
static CACHE_USAGE: AtomicU64 = AtomicU64::new(0);
static CACHE_USAGE_KNOWN: AtomicBool = AtomicBool::new(false);
static CACHE_LOCK: Mutex<()> = Mutex::new(());
static REPORT_TURN: AsyncMutex<()> = AsyncMutex::const_new(());
static RENDERER_POOL: Mutex<RendererPool> = Mutex::new(RendererPool {
    child: None,
    retirement: None,
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
            .checked_add(CACHE_HEADER_LEN)
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
        let metadata = entry.metadata()?;
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
    if total.saturating_add(incoming) > limit {
        entries.sort_by_key(|entry| entry.2);
        for (path, size, _) in entries {
            if total.saturating_add(incoming) <= limit {
                break;
            }
            std::fs::remove_file(path)?;
            total = total.saturating_sub(size);
        }
    }
    CACHE_USAGE.store(total, Ordering::SeqCst);
    CACHE_USAGE_KNOWN.store(true, Ordering::SeqCst);
    Ok(())
}

fn verify_cached(file: &mut impl Read) -> Option<(u64, Option<[u8; 32]>)> {
    let mut header = [0; 48];
    file.read_exact(&mut header).ok()?;
    let len = u64::from_le_bytes(header[8..16].try_into().ok()?);
    if len == 0 || len > LIMIT {
        return None;
    }
    let file_hash = match &header[..8] {
        tag if tag == CACHE_HEADER => {
            let mut hash = [0; 32];
            file.read_exact(&mut hash).ok()?;
            Some(hash)
        }
        tag if tag == LEGACY_CACHE_HEADER => None,
        _ => return None,
    };
    let mut digest = Sha256::new();
    let actual = io::copy(&mut file.take(len + 1), &mut digest).ok()?;
    (actual == len && digest.finalize()[..] == header[16..]).then_some((len, file_hash))
}

struct CachedAudio {
    file: std::fs::File,
    len: u64,
    file_hash: Option<[u8; 32]>,
}

impl CachedAudio {
    fn matches(&self, file_id: &str) -> bool {
        self.file_hash
            .is_some_and(|hash| hash == <[u8; 32]>::from(Sha256::digest(file_id)))
    }
}

fn open_cached(key: &str, legacy: bool) -> Option<CachedAudio> {
    if !CACHE_ENABLED.load(Ordering::SeqCst) {
        return None;
    }
    let path = cache_dir().ok()?.join(cache_name(key));
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > LIMIT + CACHE_HEADER_LEN
    {
        return None;
    }
    let mut file = std::fs::File::open(&path).ok()?;
    if let Some((len, file_hash)) = verify_cached(&mut file) {
        if file_hash.is_none() != legacy {
            return None;
        }
        let header_len = if legacy { 48 } else { CACHE_HEADER_LEN };
        file.seek(SeekFrom::Start(header_len)).ok()?;
        let _ = file.set_modified(SystemTime::now());
        Some(CachedAudio {
            file,
            len,
            file_hash,
        })
    } else {
        drop(file);
        let _ = std::fs::remove_file(&path);
        CACHE_USAGE_KNOWN.store(false, Ordering::SeqCst);
        None
    }
}

fn cache_write_allowed(len: u64, generation: u64) -> bool {
    CACHE_ENABLED.load(Ordering::SeqCst)
        && CACHE_GENERATION.load(Ordering::SeqCst) == generation
        && cache_entry_fits(len, CACHE_LIMIT.load(Ordering::SeqCst))
}

fn write_cached(
    uri: &str,
    file_id: &str,
    mut media: std::fs::File,
    len: u64,
    digest: &[u8],
    generation: u64,
) -> Result<()> {
    let _guard = CACHE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if !cache_write_allowed(len, generation) {
        return Ok(());
    }
    let directory = cache_dir()?;
    std::fs::create_dir_all(&directory)?;
    let destination = directory.join(cache_name(uri));
    if destination.is_file() {
        std::fs::remove_file(&destination)?;
    }
    let entry_size = len + CACHE_HEADER_LEN;
    trim_cache(CACHE_LIMIT.load(Ordering::SeqCst), entry_size)?;
    let temporary = directory.join(format!(
        "{}.part-{:016x}",
        cache_name(uri),
        rand::random::<u64>()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(CACHE_HEADER)?;
        file.write_all(&len.to_le_bytes())?;
        file.write_all(digest)?;
        file.write_all(&Sha256::digest(file_id))?;
        media.seek(SeekFrom::Start(0))?;
        if io::copy(&mut Read::take(media, len), &mut file)? != len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    } else {
        CACHE_USAGE.fetch_add(entry_size, Ordering::SeqCst);
    }
    result.map_err(Into::into)
}

#[derive(Clone, Copy, PartialEq)]
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
    seek_version: u64,
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
            seek_version: 0,
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
    reporting: Mutex<Option<PlaybackReporting>>,
    resources: Mutex<Option<Resources>>,
}

struct PlaybackReporting {
    tx: mpsc::UnboundedSender<PlaybackReport>,
    latest: Option<PlaybackReport>,
    loaded_at_ms: u64,
    paused: bool,
    seek_version: u64,
}

impl PlaybackReporting {
    fn seek_while_suspended(&mut self, position_ms: u64) {
        let Some(latest) = self.latest else {
            return;
        };
        let position_ms = position_ms.min(latest.duration_ms);
        if position_ms == latest.position_ms {
            return;
        }
        let report = PlaybackReport {
            kind: PlaybackReportKind::Seeked,
            position_ms,
            previous_position_ms: Some(latest.position_ms),
            paused: true,
            ..latest
        };
        let _ = self.tx.send(report);
        self.latest = Some(report);
    }

    fn observe(&mut self, state: &BrowserState) {
        let Some(mut report) = state.playback_report() else {
            return;
        };
        let previous_ms = self.latest.as_ref().map_or(0, |latest| latest.played_ms);
        if report.played_ms < previous_ms {
            return;
        }
        if state.ended {
            if let Some(latest) = self.latest {
                report.paused = latest.paused;
            }
            if let (Some(started_at_ms), Some(ended_at_ms)) =
                (state.started_at_ms, state.ended_at_ms)
            {
                report.timeline = Some(PlaybackTimeline {
                    loaded_at_ms: self.loaded_at_ms,
                    started_at_ms,
                    ended_at_ms,
                });
            }
        }
        report.previous_position_ms = self.latest.as_ref().map(|latest| latest.position_ms);
        if !state.ended
            && state.error_code.is_none()
            && state.paused != self.paused
            && (state.paused || state.playing)
        {
            self.paused = state.paused;
            let kind = if self.paused {
                PlaybackReportKind::Paused
            } else {
                PlaybackReportKind::Resumed
            };
            let _ = self.tx.send(PlaybackReport { kind, ..report });
        }
        if state.error_code.is_none() && !state.seeking && state.seek_version > self.seek_version {
            self.seek_version = state.seek_version;
            let _ = self.tx.send(PlaybackReport {
                kind: PlaybackReportKind::Seeked,
                previous_position_ms: Some(state.seek_from_ms.min(report.duration_ms)),
                ..report
            });
        }
        for (threshold, kind) in [
            (REPORT_STARTED_MS, PlaybackReportKind::Started),
            (REPORT_THRESHOLD_MS, PlaybackReportKind::ThresholdReached),
        ] {
            if previous_ms < threshold && report.played_ms >= threshold {
                let _ = self.tx.send(PlaybackReport { kind, ..report });
            }
        }
        self.latest = Some(report);
    }
}

impl Drop for PlaybackReporting {
    fn drop(&mut self) {
        if let Some(report) = self.latest.take() {
            let _ = self.tx.send(PlaybackReport {
                kind: PlaybackReportKind::Finalized,
                ..report
            });
        }
    }
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

    fn is_idle_since(&self, token: u64) -> bool {
        self.active.is_none() && self.next == token
    }
}

struct RendererPool {
    child: Option<Child>,
    retirement: Option<JoinHandle<()>>,
    ownership: RendererOwnership,
}

struct RendererLease {
    token: u64,
    reused: bool,
    runtime: tokio::runtime::Handle,
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
        if let Some(retirement) = pool.retirement.take() {
            retirement.abort();
        }
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
        Ok(Self {
            token,
            reused,
            runtime: tokio::runtime::Handle::current(),
        })
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
        let Some(child) = pool.child.as_mut() else {
            return;
        };
        if send_renderer(child, crate::spotify_drm_child::RendererCommand::Stop).is_err() {
            retire_renderer(pool.child.take().unwrap());
            return;
        }
        let token = self.token;
        let deadline = tokio::time::Instant::now() + RENDERER_IDLE_TIMEOUT;
        pool.retirement = Some(self.runtime.spawn(async move {
            tokio::time::sleep_until(deadline).await;
            let mut pool = RENDERER_POOL
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if pool.ownership.is_idle_since(token) {
                pool.retirement.take();
                if let Some(child) = pool.child.take() {
                    retire_renderer(child);
                }
            }
        }));
    }
}

pub fn retire_idle_renderer() {
    let mut pool = RENDERER_POOL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if pool.ownership.active.is_none() {
        if let Some(retirement) = pool.retirement.take() {
            retirement.abort();
        }
        if let Some(child) = pool.child.take() {
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
            reporting: Mutex::new(None),
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
        if !position.is_finite() {
            return;
        }
        if self.0.canceled.load(Ordering::SeqCst) {
            let mut reporting = self.0.reporting.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(reporter) = reporting.as_mut() {
                reporter.seek_while_suspended((position * 1000.0).round() as u64);
            }
        } else {
            self.command("seek", json!(position.max(0.0)));
        }
    }

    pub fn set_volume(&self, volume: f64) {
        if volume.is_finite() && !self.0.canceled.load(Ordering::SeqCst) {
            let volume = crate::player::volume_gain(volume) * 100.0;
            self.0
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .volume = volume;
            self.command("volume", json!(volume));
        }
    }

    pub fn stop(&self) {
        self.release_renderer(false);
    }

    pub fn suspend(&self) {
        self.release_renderer(true);
    }

    fn release_renderer(&self, keep_reporting: bool) {
        self.0.canceled.store(true, Ordering::SeqCst);
        if !keep_reporting {
            self.0
                .reporting
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
        }
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
    uri: Arc<str>,
    playback: watch::Receiver<Option<Arc<ValidatedPlayback>>>,
    media: AsyncMutex<JoinSet<Option<CachedAudio>>>,
    prefix: String,
    origin: String,
    volume: f64,
    start_at: Option<f64>,
    cache_generation: u64,
    handle: Weak<Inner>,
}

struct ValidatedPlayback {
    track: PreparedTrack,
    certificate: Vec<u8>,
}

impl Broker {
    async fn validated_playback(&self) -> Result<Arc<ValidatedPlayback>> {
        let mut playback = self.playback.clone();
        let ready = playback.wait_for(Option::is_some).await?;
        Ok(ready.as_ref().unwrap().clone())
    }
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
        let waiting = Instant::now();
        let report_turn = REPORT_TURN.lock().await;
        let report_wait_ms = waiting.elapsed().as_millis();
        let started = Instant::now();
        let track = transport.prepare_for_playback(uri).await?;
        Result::<_>::Ok((
            track,
            started.elapsed().as_millis(),
            report_wait_ms,
            report_turn,
        ))
    };
    let service_certificate = async {
        let started = Instant::now();
        let value = transport.service_certificate().await?;
        let certificate = certificate(&value)
            .context("Spotify returned an invalid service certificate")?
            .to_vec();
        Result::<_>::Ok((certificate, started.elapsed().as_millis()))
    };
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let origin = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let prefix = format!("/{}/", capability());
    let url = format!("{origin}{prefix}");
    let (validated, playback) = watch::channel(None);
    let uri_key: Arc<str> = Arc::from(uri);
    let mut media = JoinSet::new();
    if CACHE_ENABLED.load(Ordering::SeqCst) {
        let uri_key = uri_key.clone();
        let active = Arc::downgrade(&handle.0);
        media.spawn_blocking(move || {
            let _guard = CACHE_LOCK.try_lock().ok()?;
            if CACHE_GENERATION.load(Ordering::SeqCst) != cache_generation
                || active.upgrade()?.canceled.load(Ordering::SeqCst)
            {
                return None;
            }
            open_cached(&uri_key, false)
        });
    }
    let broker = Arc::new(Broker {
        transport: transport.clone(),
        uri: uri_key,
        playback,
        media: AsyncMutex::new(media),
        prefix,
        origin,
        volume: crate::player::volume_gain(volume) * 100.0,
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
    let renderer_reused = {
        let mut resources = handle.0.resources.lock().unwrap_or_else(|e| e.into_inner());
        let renderer = match RendererLease::acquire(&url, &handle) {
            Ok(renderer) => renderer,
            Err(error) => {
                server.abort();
                return Err(error);
            }
        };
        let reused = renderer.reused;
        *resources = Some(Resources { server, renderer });
        reused
    };
    let renderer_started = Instant::now();
    let ((track, prepare_ms, report_wait_ms, report_turn), (certificate, certificate_ms)) =
        tokio::try_join!(prepare, service_certificate)?;
    if let Some(error) = handle.last_error() {
        bail!(error);
    }
    ensure!(
        !handle.0.canceled.load(Ordering::SeqCst),
        "Spotify playback canceled"
    );
    let playback = Arc::new(ValidatedPlayback { track, certificate });
    if playback.track.can_report_playback() {
        let mut reporting = handle.0.reporting.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(
            !handle.0.canceled.load(Ordering::SeqCst),
            "Spotify playback canceled"
        );
        let (tx, mut reports) = mpsc::unbounded_channel::<PlaybackReport>();
        let start_offset_ms = (start_at.unwrap_or_default() * 1000.0).round() as u64;
        let paused = handle.0.paused.load(Ordering::SeqCst);
        let _ = tx.send(PlaybackReport {
            kind: PlaybackReportKind::Loading,
            position_ms: start_offset_ms,
            previous_position_ms: None,
            duration_ms: playback.track.metadata.duration_ms.unwrap_or_default(),
            played_ms: 0,
            start_offset_ms,
            paused,
            playback_speed: 1.0,
            timeline: None,
        });
        *reporting = Some(PlaybackReporting {
            tx,
            latest: None,
            loaded_at_ms: crate::spotify::now_ms(),
            paused,
            seek_version: 0,
        });
        let report_playback = playback.clone();
        tokio::spawn(async move {
            let _turn = report_turn;
            let mut next = None;
            while let Some(mut report) = match next.take() {
                Some(report) => Some(report),
                None => reports.recv().await,
            } {
                if report.kind == PlaybackReportKind::Seeked {
                    let previous_position = report.previous_position_ms;
                    while let Ok(queued) = reports.try_recv() {
                        if queued.kind != PlaybackReportKind::Seeked {
                            next = Some(queued);
                            break;
                        }
                        report = queued;
                    }
                    report.previous_position_ms = previous_position;
                }
                let kind = report.kind;
                let authorization = match tokio::time::timeout(
                    REPORT_AUTH_TIMEOUT,
                    crate::spotify::transport(),
                )
                .await
                {
                    Ok(Ok(authorization)) => authorization,
                    Ok(Err(error)) => {
                        tracing::warn!(?kind, %error, "Spotify playback report authorization failed");
                        continue;
                    }
                    Err(_) => {
                        tracing::warn!(?kind, "Spotify playback report authorization timed out");
                        continue;
                    }
                };
                let track = &report_playback.track;
                let verification = async {
                    if kind == PlaybackReportKind::Started {
                        match transport
                            .report_playback_start(&authorization, track, report)
                            .await
                        {
                            Ok(true) => tracing::debug!("Spotify playback start summary sent"),
                            Ok(false) => tracing::debug!("Spotify playback start summary skipped"),
                            Err(error) => {
                                tracing::warn!(%error, "Spotify playback start summary failed")
                            }
                        }
                    } else if kind == PlaybackReportKind::Finalized {
                        let (stream, media, stats) = tokio::join!(
                            transport.report_stream_verification(&authorization, track, report),
                            transport.report_media_playback(&authorization, track, report),
                            transport.report_playback_stats(&authorization, track, report)
                        );
                        match stream {
                            Ok(true) => tracing::debug!("Spotify stream verification sent"),
                            Ok(false) => tracing::debug!("Spotify stream verification skipped"),
                            Err(error) => {
                                tracing::warn!(%error, "Spotify stream verification failed")
                            }
                        }
                        match media {
                            Ok(true) => tracing::debug!("Spotify media end report sent"),
                            Ok(false) => tracing::debug!("Spotify media end report skipped"),
                            Err(error) => tracing::warn!(%error, "Spotify media end report failed"),
                        }
                        match stats {
                            Ok(true) => tracing::debug!("Spotify playback stats sent"),
                            Ok(false) => tracing::debug!("Spotify playback stats skipped"),
                            Err(error) => tracing::warn!(%error, "Spotify playback stats failed"),
                        }
                    }
                };
                let (state_report, ()) = tokio::join!(
                    transport.report_playback(&authorization, track, report),
                    verification
                );
                match state_report {
                    Ok(true) => tracing::debug!(?kind, "Spotify playback report sent"),
                    Ok(false) => tracing::debug!(?kind, "Spotify playback report skipped"),
                    Err(error) => tracing::warn!(?kind, %error, "Spotify playback report failed"),
                }
            }
        });
    } else {
        drop(report_turn);
        tracing::debug!("Spotify playback reporting is unavailable for this track");
    }
    validated.send_replace(Some(playback));
    let deadline =
        Instant::now() + MEDIA_DOWNLOAD_TIMEOUT + RENDERER_RESPONSE_TIMEOUT + PLAYBACK_READY_GRACE;
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
                report_wait_ms,
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

async fn read_request_header(input: impl AsyncBufRead + Unpin) -> Result<Vec<u8>> {
    let mut input = input.take(8192);
    let mut request = Vec::new();
    while input.read_until(b'\n', &mut request).await? != 0 {
        if request.ends_with(b"\r\n\r\n") {
            return Ok(request);
        }
    }
    bail!("Invalid renderer request");
}

async fn serve(mut socket: TcpStream, broker: Arc<Broker>) -> Result<()> {
    let mut input = BufReader::with_capacity(1024, &mut socket);
    let request =
        tokio::time::timeout(Duration::from_secs(5), read_request_header(&mut input)).await??;
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
        tokio::time::timeout(Duration::from_secs(10), input.read_exact(&mut body)).await??;
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
            let playback = broker.validated_playback().await?;
            let meta = serde_json::to_vec(&json!({
                "volume": broker.volume,
                "startAt": broker.start_at,
                "duration": playback.track.metadata.duration_ms.map(|ms| ms as f64 / 1000.0),
            }))?;
            respond(&mut socket, 200, "application/json", &meta).await
        }
        ("GET", "certificate") => {
            let playback = broker.validated_playback().await?;
            respond(
                &mut socket,
                200,
                "application/octet-stream",
                &playback.certificate,
            )
            .await
        }
        ("GET", "media") => {
            let playback = broker.validated_playback().await?;
            let mut response_started = false;
            match tokio::time::timeout(
                MEDIA_DOWNLOAD_TIMEOUT,
                serve_media(&mut socket, &broker, &playback.track, &mut response_started),
            )
            .await
            {
                Ok(Ok(())) => Ok(()),
                _ if !response_started => respond(&mut socket, 502, "text/plain", b"").await,
                Ok(Err(error)) => Err(error),
                Err(error) => Err(error.into()),
            }
        }
        ("POST", "license") => {
            broker.validated_playback().await?;
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
            tokio::pin!(notified);
            notified.as_mut().enable();
            let empty = inner
                .commands
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty();
            if empty {
                let _ = tokio::time::timeout(Duration::from_secs(20), notified).await;
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
            let finished = state.ended || state.error_code.is_some();
            state.update(&inner)?;
            let result = respond(&mut socket, 200, "text/plain", b"").await;
            if finished {
                inner
                    .resources
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take();
            }
            result
        }
        _ => respond(&mut socket, 404, "text/plain", b"").await,
    }
}

async fn serve_media(
    socket: &mut TcpStream,
    broker: &Broker,
    track: &PreparedTrack,
    response_started: &mut bool,
) -> Result<()> {
    let mut preloaded = broker.media.lock().await;
    let started = Instant::now();
    let preloaded = preloaded.join_next().await.and_then(Result::ok).flatten();
    let warmed = preloaded
        .as_ref()
        .is_some_and(|audio| audio.matches(&track.file_id));
    let cached = if warmed {
        preloaded
    } else {
        let file_id = track.file_id.clone();
        let uri = broker.uri.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = CACHE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
            if preloaded.is_none()
                && let Some(audio) = open_cached(&uri, false)
                && audio.matches(&file_id)
            {
                return Some(audio);
            }
            open_cached(&file_id, true)
        })
        .await
        .ok()
        .flatten()
    };
    if let Some(CachedAudio { file, len, .. }) = cached
        && CACHE_GENERATION.load(Ordering::SeqCst) == broker.cache_generation
    {
        *response_started = true;
        respond_head(socket, 200, "audio/mp4", len).await?;
        let mut media = AsyncReadExt::take(tokio::fs::File::from_std(file), len);
        ensure!(
            tokio::io::copy(&mut media, socket).await? == len,
            "Spotify cache entry changed during playback"
        );
        track.record_playback_media(len, true);
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            cache_hit = true,
            cache_preloaded = warmed,
            "Spotify media ready"
        );
        return Ok(());
    }

    let mut range = broker
        .transport
        .fetch_range(track, 0, FIRST_MEDIA_RANGE - 1)
        .await?;
    let total = range.total_len;
    ensure!(
        (1..=LIMIT).contains(&total),
        "Spotify media exceeds the 64 MiB limit"
    );
    check_media_range(&range, 0, total)?;
    *response_started = true;
    respond_head(socket, 200, "audio/mp4", total).await?;
    socket.write_all(&range.bytes).await?;
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        "Spotify media started"
    );

    let generation = broker.cache_generation;
    let mut cache = if cache_write_allowed(total, generation) {
        tokio::task::spawn_blocking(tempfile::tempfile)
            .await
            .ok()
            .and_then(Result::ok)
            .map(tokio::fs::File::from_std)
    } else {
        None
    };
    let mut digest = Sha256::new();
    let mut offset = 0;
    loop {
        if let Some(file) = &mut cache {
            if file.write_all(&range.bytes).await.is_ok() {
                digest.update(&range.bytes);
            } else {
                cache = None;
            }
        }
        offset += range.bytes.len() as u64;
        if offset == total {
            break;
        }
        drop(range);
        range = broker
            .transport
            .fetch_range(track, offset, (offset + MEDIA_RANGE - 1).min(total - 1))
            .await?;
        check_media_range(&range, offset, total)?;
        socket.write_all(&range.bytes).await?;
    }
    track.record_playback_media(total, false);
    if let Some(mut file) = cache
        && file.flush().await.is_ok()
    {
        let file = file.into_std().await;
        let uri = broker.uri.clone();
        let file_id = track.file_id.clone();
        let digest = digest.finalize();
        drop(tokio::task::spawn_blocking(move || {
            write_cached(&uri, &file_id, file, total, &digest, generation)
        }));
    }
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        cache_hit = false,
        "Spotify media ready"
    );
    Ok(())
}

fn check_media_range(range: &RangeData, offset: u64, total: u64) -> Result<()> {
    ensure!(
        range.total_len == total
            && !range.bytes.is_empty()
            && offset + range.bytes.len() as u64 <= total,
        "Spotify media changed during playback"
    );
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserState {
    position: f64,
    duration: f64,
    played_ms: u64,
    start_offset_ms: u64,
    started_at_ms: Option<u64>,
    ended_at_ms: Option<u64>,
    seek_version: u64,
    seek_from_ms: u64,
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
    fn playback_report(&self) -> Option<PlaybackReport> {
        if self.played_ms == 0 || self.duration <= 0.0 {
            return None;
        }
        let duration_ms = (self.duration * 1000.0).round() as u64;
        Some(PlaybackReport {
            kind: PlaybackReportKind::Finalized,
            position_ms: ((self.position * 1000.0).round() as u64).min(duration_ms),
            previous_position_ms: None,
            duration_ms,
            played_ms: self.played_ms.min(duration_ms),
            start_offset_ms: self.start_offset_ms.min(duration_ms),
            paused: self.paused,
            playback_speed: self.rate,
            timeline: None,
        })
    }

    fn update(&self, inner: &Inner) -> Result<()> {
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
        {
            let mut reporting = inner.reporting.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(reporter) = reporting.as_mut() {
                reporter.observe(self);
            }
            if self.ended || self.error_code.is_some() {
                reporting.take();
            }
        }
        let next = Snapshot {
            position: self.position,
            duration: self.duration,
            paused: self.paused,
            idle: self.ended && self.error_code.is_none(),
            buffering: self.buffering,
            seeking: self.seeking,
            rate: self.rate,
            volume: self.volume,
            muted: self.muted,
            seek_version: self.seek_version,
        };
        let changed = {
            let mut previous = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let position = if self.playing && !self.seeking {
                previous.position
            } else {
                next.position
            };
            let changed = *previous != Snapshot { position, ..next };
            *previous = next;
            changed
        };
        let ready =
            self.playing || (self.ready && self.paused && inner.paused.load(Ordering::SeqCst));
        let became_ready = ready && !inner.playing.swap(true, Ordering::SeqCst);
        inner.started.notify_waiters();
        if changed || became_ready || self.error_code.is_some() {
            inner.ctx.request_repaint();
        }
        Ok(())
    }
}

async fn respond(socket: &mut TcpStream, status: u16, mime: &str, body: &[u8]) -> Result<()> {
    tokio::time::timeout(RENDERER_RESPONSE_TIMEOUT, async {
        respond_head(socket, status, mime, body.len() as u64).await?;
        socket.write_all(body).await
    })
    .await??;
    Ok(())
}

async fn respond_head(socket: &mut TcpStream, status: u16, mime: &str, len: u64) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} Response\r\nContent-Type: {mime}\r\nContent-Length: {len}\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; connect-src 'self'; media-src blob:; base-uri 'none'; frame-ancestors 'none'\r\n\r\n",
    );
    socket.write_all(head.as_bytes()).await
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

    #[tokio::test]
    async fn renderer_headers_keep_body_bytes_and_enforce_the_size_limit() {
        let mut request = b"POST /state HTTP/1.1\r\nX-Fill: ".to_vec();
        request.resize(8192 - 4, b'x');
        request.extend_from_slice(b"\r\n\r\nbody");
        let mut input = BufReader::with_capacity(request.len(), request.as_slice());
        assert_eq!(read_request_header(&mut input).await.unwrap().len(), 8192);
        let mut body = [0; 4];
        input.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"body");

        request.insert(8192 - 4, b'x');
        assert!(
            read_request_header(BufReader::new(request.as_slice()))
                .await
                .is_err()
        );
        assert!(
            read_request_header(BufReader::new(&b"GET / HTTP/1.1\r\n"[..]))
                .await
                .is_err()
        );
    }

    #[test]
    fn renderer_progress_updates_the_snapshot_without_requesting_frames() {
        let ctx = egui::Context::default();
        let handle = Handle::new(ctx.clone());
        let repaint_requested = || {
            let _ = ctx.run_logic(&egui::RawInput::default(), |_| {});
            !ctx.repaint_causes().is_empty()
        };
        let mut state = BrowserState {
            position: 10.0,
            duration: 100.0,
            played_ms: 10_000,
            start_offset_ms: 0,
            started_at_ms: Some(1_000),
            ended_at_ms: None,
            seek_version: 0,
            seek_from_ms: 0,
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
        };
        state.update(&handle.0).unwrap();
        assert!(repaint_requested());

        state.position = 10.25;
        state.played_ms = 10_250;
        state.update(&handle.0).unwrap();
        assert_eq!(handle.state().position, 10.25);
        assert!(!repaint_requested());

        state.position = 50.0;
        state.seek_version = 1;
        state.seek_from_ms = 10_250;
        state.update(&handle.0).unwrap();
        assert!(repaint_requested());

        handle.set_paused(true);
        state.position = 50.1;
        state.paused = true;
        state.playing = false;
        state.update(&handle.0).unwrap();
        assert!(repaint_requested());

        state.error_code = Some(3);
        state.update(&handle.0).unwrap();
        assert!(repaint_requested());
        assert!(handle.last_error().is_some());
    }

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
        assert!(ownership.is_idle_since(first));
        let second = ownership.acquire(false).unwrap();
        assert!(!ownership.is_idle_since(first));
        assert!(!ownership.release(first));
        assert_eq!(ownership.active, Some(second));
        assert!(ownership.release(second));
        assert!(!ownership.is_idle_since(first));
        assert!(ownership.is_idle_since(second));
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
        assert!(cache_entry_fits(limit - CACHE_HEADER_LEN, limit));
        assert!(!cache_entry_fits(limit - CACHE_HEADER_LEN + 1, limit));
        assert!(!cache_entry_fits(limit, limit));
        assert!(!cache_entry_fits(0, limit));
        assert!(!cache_entry_fits(LIMIT + 1, cache_limit_bytes(4096)));
        assert!(cache_entry_fits(LIMIT, LIMIT + CACHE_HEADER_LEN));
    }

    #[test]
    fn stopped_playback_rejects_late_renderer_state() {
        let handle = Handle::new(egui::Context::default());
        handle.stop();
        BrowserState {
            position: 50.0,
            duration: 100.0,
            played_ms: 0,
            start_offset_ms: 0,
            started_at_ms: None,
            ended_at_ms: None,
            seek_version: 0,
            seek_from_ms: 0,
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
    fn spotify_reports_follow_played_audio_instead_of_seek_position() {
        let (tx, mut reports) = mpsc::unbounded_channel();
        let mut reporter = PlaybackReporting {
            tx,
            latest: None,
            loaded_at_ms: 1_000,
            paused: false,
            seek_version: 0,
        };
        let mut state = BrowserState {
            position: 60.0,
            duration: 100.0,
            played_ms: 0,
            start_offset_ms: 60_000,
            started_at_ms: None,
            ended_at_ms: None,
            seek_version: 0,
            seek_from_ms: 0,
            paused: false,
            ended: false,
            buffering: false,
            seeking: true,
            rate: 1.0,
            volume: 70.0,
            muted: false,
            playing: false,
            ready: true,
            error_code: None,
        };
        reporter.observe(&state);
        assert!(reports.try_recv().is_err());
        state.seeking = false;
        state.playing = true;
        state.position = 61.25;
        state.played_ms = 1_250;
        state.started_at_ms = Some(2_000);
        reporter.observe(&state);
        let started = reports.try_recv().unwrap();
        assert!(matches!(started.kind, PlaybackReportKind::Started));
        assert_eq!(started.start_offset_ms, 60_000);
        state.paused = true;
        state.playing = false;
        reporter.observe(&state);
        let paused = reports.try_recv().unwrap();
        assert_eq!(paused.kind, PlaybackReportKind::Paused);
        assert!(paused.paused);
        assert_eq!(paused.played_ms, 1_250);
        reporter.observe(&state);
        assert!(reports.try_recv().is_err());
        state.position = 80.0;
        state.seek_from_ms = 61_250;
        state.seek_version = 1;
        state.seeking = true;
        reporter.observe(&state);
        assert!(reports.try_recv().is_err());
        state.seeking = false;
        reporter.observe(&state);
        let seeked = reports.try_recv().unwrap();
        assert_eq!(seeked.kind, PlaybackReportKind::Seeked);
        assert_eq!(seeked.previous_position_ms, Some(61_250));
        assert_eq!(seeked.position_ms, 80_000);
        assert_eq!(seeked.played_ms, 1_250);
        assert!(seeked.paused);
        state.paused = false;
        reporter.observe(&state);
        assert!(reports.try_recv().is_err());
        state.playing = true;
        reporter.observe(&state);
        let resumed = reports.try_recv().unwrap();
        assert_eq!(resumed.kind, PlaybackReportKind::Resumed);
        assert!(!resumed.paused);
        state.position = 100.0;
        state.played_ms = 31_500;
        state.ended = true;
        state.paused = true;
        state.ended_at_ms = Some(35_000);
        reporter.observe(&state);
        drop(reporter);
        let threshold = reports.try_recv().unwrap();
        assert!(matches!(
            threshold.kind,
            PlaybackReportKind::ThresholdReached
        ));
        let finalized = reports.try_recv().unwrap();
        assert!(matches!(finalized.kind, PlaybackReportKind::Finalized));
        assert_eq!(finalized.played_ms, 31_500);
        assert_eq!(finalized.position_ms, 100_000);
        assert!(!finalized.paused);
        assert_eq!(finalized.timeline.unwrap().ended_at_ms, 35_000);
        assert!(reports.try_recv().is_err());
    }

    #[test]
    fn suspended_spotify_seeks_keep_reported_listening_time() {
        let handle = Handle::new(egui::Context::default());
        let (tx, mut reports) = mpsc::unbounded_channel();
        *handle.0.reporting.lock().unwrap() = Some(PlaybackReporting {
            tx,
            latest: Some(PlaybackReport {
                kind: PlaybackReportKind::Paused,
                position_ms: 12_000,
                previous_position_ms: None,
                duration_ms: 90_000,
                played_ms: 700,
                start_offset_ms: 11_300,
                paused: true,
                playback_speed: 1.0,
                timeline: None,
            }),
            loaded_at_ms: 1_000,
            paused: true,
            seek_version: 0,
        });
        handle.suspend();
        handle.seek(45.0);
        let seek = reports.try_recv().unwrap();
        assert_eq!(seek.kind, PlaybackReportKind::Seeked);
        assert_eq!(seek.previous_position_ms, Some(12_000));
        assert_eq!(seek.position_ms, 45_000);
        assert_eq!(seek.played_ms, 700);
        assert!(seek.paused);
        handle.stop();
        let finalized = reports.try_recv().unwrap();
        assert_eq!(finalized.kind, PlaybackReportKind::Finalized);
        assert_eq!(finalized.played_ms, 700);
        assert!(reports.try_recv().is_err());
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
        let file_hash: [u8; 32] = Sha256::digest("spotify-file").into();
        encoded.extend_from_slice(&file_hash);
        encoded.extend_from_slice(media);
        assert_eq!(
            verify_cached(&mut encoded.as_slice()),
            Some((media.len() as u64, Some(file_hash)))
        );
        let mut legacy = Vec::from(LEGACY_CACHE_HEADER.as_slice());
        legacy.extend_from_slice(&encoded[8..48]);
        legacy.extend_from_slice(media);
        assert_eq!(
            verify_cached(&mut legacy.as_slice()),
            Some((media.len() as u64, None))
        );
        for length in [0, 8, 16, 47, 79, encoded.len() - 1] {
            assert!(verify_cached(&mut &encoded[..length]).is_none());
        }
        encoded.push(0);
        assert!(verify_cached(&mut encoded.as_slice()).is_none());
        encoded.pop();
        *encoded.last_mut().unwrap() ^= 1;
        assert!(verify_cached(&mut encoded.as_slice()).is_none());
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
    fn volume_commands_apply_logarithmic_gain() {
        let handle = Handle::new(egui::Context::default());
        for (volume, gain) in [
            (-10.0, 0.0),
            (0.0, 0.0),
            (1.0, 0.001071519305237606),
            (50.0, 0.03162277660168379),
            (100.0, 1.0),
            (150.0, 1.0),
        ] {
            handle.set_volume(volume);
            let command = handle.0.commands.lock().unwrap().pop_front().unwrap();
            assert_eq!(command["kind"], "volume");
            let sent = command["value"].as_f64().unwrap();
            assert!((sent / 100.0 - gain).abs() < 1e-12);
            assert_eq!(handle.state().volume, sent);
        }
        for volume in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            handle.set_volume(volume);
            assert!(handle.0.commands.lock().unwrap().is_empty());
            assert_eq!(handle.state().volume, 100.0);
        }
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
