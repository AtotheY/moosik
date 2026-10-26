//! Private, append-only listening telemetry. No credentials or stream URLs enter this module.
//! The UI samples mpv; a single worker owns SQLite and exports. Nothing is uploaded.
pub mod report;
mod store;
use crate::model::Track;
use anyhow::Result;
use chrono::{Datelike, Local, Timelike};
pub use report::{Group, Query, Recent, Report, Row};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

pub fn path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "refryt")
        .ok_or_else(|| anyhow::anyhow!("Could not locate the app data directory"))?;
    Ok(dirs.data_dir().join("listening/listening.sqlite3"))
}
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub id: String,
    pub title: String,
    pub artists: Vec<(String, Option<String>)>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub duration: f64,
    pub explicit: bool,
    pub liked: bool,
    pub video: bool,
    pub artwork: Option<String>,
}
impl Metadata {
    pub fn from_track(t: &Track) -> Self {
        Self {
            id: t.video_id.clone(),
            title: t.title.clone(),
            artists: t
                .artists
                .iter()
                .map(|a| (a.name.clone(), a.id.clone()))
                .collect(),
            album: t.album.clone(),
            album_id: t.album_id.clone(),
            duration: t.duration.unwrap_or(0) as f64,
            explicit: t.explicit,
            liked: t.like == crate::model::LikeStatus::Like,
            video: t.wide,
            artwork: t.thumbnail.as_deref().map(str::to_owned),
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Context {
    pub source: String,
    pub route: String,
    pub playlist: Option<String>,
    pub shuffle: bool,
    pub repeat: String,
    pub volume: f64,
    pub video: bool,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Sample {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub idle: bool,
    pub buffering: bool,
    pub seeking: bool,
    pub speed: f64,
    pub volume: f64,
    pub muted: bool,
    pub seek_serial: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub session: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub from: f64,
    pub to: f64,
    pub listen_ms: f64,
    pub media_ms: f64,
    pub paused_ms: f64,
    pub buffer_ms: f64,
    pub muted_ms: f64,
    pub unobserved_ms: f64,
    pub day: String,
    pub hour: u32,
    pub weekday: u32,
    pub offset_seconds: i32,
    pub volume: f64,
    pub speed: f64,
    pub coverage_ms: f64,
    pub duration: f64,
}
#[derive(Clone, Debug)]
pub enum Record {
    Start {
        id: String,
        at: i64,
        track: Metadata,
        context: Context,
    },
    Event {
        id: String,
        at: i64,
        kind: String,
        payload: serde_json::Value,
    },
    Segment(Segment),
    End {
        id: String,
        at: i64,
        reason: String,
    },
}
#[derive(Clone, Copy, Debug)]
pub enum Export {
    Csv,
    Json,
    Recap,
    Database,
}
enum Command {
    Records(Vec<Record>),
    Report(Query),
    Tags(String, String, String, i32),
    Export(Export, PathBuf, Query),
    Shutdown,
}
enum Reply {
    Report(Box<Report>),
    Error(String),
    Saved(String),
}

/// Position ranges heard, independent of replays/seeks. Prevents 30s replayed four times
/// from counting as 100% coverage of a two-minute track.
#[derive(Default)]
struct Coverage(Vec<(f64, f64)>);
impl Coverage {
    fn add(&mut self, from: f64, to: f64) -> f64 {
        if to > from {
            self.0.push((from.max(0.0), to));
        }
        self.0.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for &(a, b) in &self.0 {
            if let Some(last) = merged.last_mut()
                && a <= last.1
            {
                last.1 = last.1.max(b);
            } else {
                merged.push((a, b));
            }
        }
        self.0 = merged;
        self.0.iter().map(|(a, b)| b - a).sum::<f64>() * 1000.0
    }
}
struct Active {
    id: String,
    previous: Option<(Sample, Instant, i64)>,
    coverage: Coverage,
    ready: bool,
    expected_start: f64,
    armed: bool,
}
/// Conservatively credit only demonstrated forward progress. Seeks, sleep, buffering and
/// app mute earn no listening time. Audio-device/system mute is outside mpv's visibility.
fn credit(prev: Sample, next: Sample, elapsed: f64) -> (f64, f64) {
    if !(0.0..=10.0).contains(&elapsed)
        || elapsed == 0.0
        || !elapsed.is_finite()
        || prev.paused
        || next.paused
        || prev.idle
        || next.idle
        || prev.buffering
        || next.buffering
        || prev.seeking
        || next.seeking
        || prev.seek_serial != next.seek_serial
        || prev.muted
        || next.muted
        || prev.volume <= 0.0
        || next.volume <= 0.0
    {
        return (0.0, 0.0);
    }
    let advance = next.position - prev.position;
    let speed = if next.speed.is_finite() && next.speed > 0.0 {
        next.speed
    } else {
        1.0
    };
    if !advance.is_finite() || advance <= 0.0 || (advance - elapsed * speed).abs() > 1.25 {
        return (0.0, 0.0);
    }
    let wall = elapsed.min(advance / speed);
    (wall * 1000.0, (wall * speed) * 1000.0)
}

pub struct Ledger {
    tx: mpsc::Sender<Command>,
    rx: mpsc::Receiver<Reply>,
    worker: Option<std::thread::JoinHandle<()>>,
    active: Option<Active>,
    pending: Vec<Record>,
    last_flush: Instant,
    last_sample: Instant,
    nonce: u64,
    pub report: Option<Report>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub loading: bool,
    requested: Option<Query>,
    requested_at: Instant,
}
impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}
impl Ledger {
    pub fn new() -> Self {
        let (tx, commands) = mpsc::channel();
        let (replies, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("moosik-listening".into())
            .spawn(move || {
                if let Err(e) = store::worker(commands, &replies) {
                    let _ = replies.send(Reply::Error(format!(
                        "Listening storage unavailable: {e:#}"
                    )));
                }
            });
        let error = worker.as_ref().err().map(|e| e.to_string());
        Self {
            tx,
            rx,
            worker: worker.ok(),
            active: None,
            pending: Vec::new(),
            last_flush: Instant::now(),
            last_sample: Instant::now(),
            nonce: 0,
            report: None,
            error,
            notice: None,
            loading: false,
            requested: None,
            requested_at: Instant::now() - Duration::from_secs(60),
        }
    }
    pub fn start(&mut self, track: &Track, context: Context, start: f64) {
        self.finish("replaced");
        self.nonce += 1;
        let at = now_ms();
        let id = format!("{at}-{}-{}", std::process::id(), self.nonce);
        self.pending.push(Record::Start {
            id: id.clone(),
            at,
            track: Metadata::from_track(track),
            context,
        });
        self.active = Some(Active {
            id,
            previous: None,
            coverage: Coverage::default(),
            ready: false,
            expected_start: start,
            armed: false,
        });
        self.flush();
    }
    pub fn ready(&mut self) {
        if let Some(a) = &mut self.active {
            a.ready = true;
        }
    }
    pub fn suspend(&mut self, start: f64, kind: &str) {
        self.event(kind, serde_json::json!({"position":start}));
        if let Some(a) = &mut self.active {
            a.ready = false;
            a.previous = None;
            a.armed = false;
            a.expected_start = start;
        }
        self.flush();
    }
    pub fn event(&mut self, kind: &str, payload: serde_json::Value) {
        if let Some(a) = &self.active {
            self.pending.push(Record::Event {
                id: a.id.clone(),
                at: now_ms(),
                kind: kind.into(),
                payload,
            });
        }
    }
    pub fn due(&self) -> bool {
        self.active.is_some() && self.last_sample.elapsed() >= Duration::from_secs(1)
    }
    pub fn active(&self) -> bool {
        self.active.is_some()
    }
    pub fn observe(&mut self, sample: Sample) {
        self.last_sample = Instant::now();
        let Some(a) = &mut self.active else { return };
        if !a.ready {
            return;
        }
        // loadfile is asynchronous: do not attribute an outgoing stream's position to the
        // new track. Arm only near this load's requested start.
        if !a.armed {
            if sample.idle || sample.seeking || (sample.position - a.expected_start).abs() > 8.0 {
                return;
            }
            a.armed = true;
        }
        let mono = Instant::now();
        let at = now_ms();
        if let Some((prev, then, wall)) = a.previous {
            let elapsed = mono.duration_since(then).as_secs_f64();
            let (listen, media) = credit(prev, sample, elapsed);
            let mut events = Vec::new();
            if prev.paused != sample.paused {
                events.push(if sample.paused { "pause" } else { "resume" });
            }
            if prev.buffering != sample.buffering {
                events.push(if sample.buffering {
                    "buffer_start"
                } else {
                    "buffer_end"
                });
            }
            if prev.seek_serial != sample.seek_serial
                || (!sample.idle
                    && !prev.idle
                    && ((sample.position - prev.position) < -0.5
                        || (sample.position - prev.position)
                            > elapsed * sample.speed.max(1.0) + 1.25))
            {
                events.push("seek");
            }
            if prev.muted != sample.muted || (prev.volume - sample.volume).abs() > 0.5 {
                events.push("volume");
            }
            if elapsed > 10.0 {
                events.push("observation_gap");
            }
            for kind in events {
                self.pending.push(Record::Event { id:a.id.clone(), at, kind:kind.into(),
                    payload:serde_json::json!({"from":prev.position,"to":sample.position,"state":sample,"elapsed_seconds":elapsed}) });
            }
            let local = Local::now();
            let observable = elapsed <= 10.0
                && (at - wall) >= 0
                && ((at - wall) as f64 - elapsed * 1000.0).abs() < 2000.0;
            let ms = elapsed * 1000.0;
            let coverage = a.coverage.add(
                sample.position - media / 1000.0,
                if observable && listen > 0.0 {
                    sample.position
                } else {
                    sample.position - media / 1000.0
                },
            );
            self.pending.push(Record::Segment(Segment {
                session: a.id.clone(),
                start_ms: wall,
                end_ms: at,
                from: prev.position,
                to: sample.position,
                listen_ms: if observable { listen } else { 0.0 },
                media_ms: if observable { media } else { 0.0 },
                paused_ms: if observable && prev.paused && sample.paused {
                    ms
                } else {
                    0.0
                },
                buffer_ms: if observable
                    && !prev.paused
                    && !sample.paused
                    && prev.buffering
                    && sample.buffering
                {
                    ms
                } else {
                    0.0
                },
                muted_ms: if observable
                    && !sample.paused
                    && !sample.idle
                    && !sample.buffering
                    && (sample.muted || sample.volume <= 0.0)
                {
                    ms
                } else {
                    0.0
                },
                unobserved_ms: if observable { 0.0 } else { ms },
                day: local.format("%Y-%m-%d").to_string(),
                hour: local.hour(),
                weekday: local.weekday().num_days_from_monday(),
                offset_seconds: local.offset().local_minus_utc(),
                volume: sample.volume,
                speed: sample.speed,
                coverage_ms: coverage,
                duration: sample.duration,
            }));
        }
        a.previous = Some((sample, mono, at));
        if self.last_flush.elapsed() >= Duration::from_secs(5) {
            self.flush();
        }
    }
    pub fn finish(&mut self, reason: &str) {
        if let Some(a) = self.active.take() {
            self.pending.push(Record::End {
                id: a.id,
                at: now_ms(),
                reason: reason.into(),
            });
        }
        self.flush();
    }
    pub fn flush(&mut self) {
        if !self.pending.is_empty() {
            let records = std::mem::take(&mut self.pending);
            if self.tx.send(Command::Records(records)).is_err() {
                self.error = Some(
                    "Listening recorder stopped; new plays are not being saved. Restart Moosik."
                        .into(),
                );
            }
        }
        self.last_flush = Instant::now();
    }
    pub fn poll(&mut self) {
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::Report(report) => {
                    self.report = Some(*report);
                    self.loading = false;
                }
                Reply::Error(e) => {
                    self.error = Some(e);
                    self.loading = false;
                }
                Reply::Saved(s) => self.notice = Some(s),
            }
        }
    }
    pub fn request(&mut self, query: Query, force: bool) {
        self.poll();
        if !force
            && (self.loading
                || (self.requested.as_ref() == Some(&query)
                    && self.requested_at.elapsed() < Duration::from_secs(10)))
        {
            return;
        }
        self.flush();
        self.loading = true;
        self.requested = Some(query.clone());
        self.requested_at = Instant::now();
        if self.tx.send(Command::Report(query)).is_err() {
            self.loading = false;
            self.error = Some("Listening database is offline. Restart Moosik.".into());
        }
    }
    pub fn tags(&mut self, id: String, genres: String, tags: String, rating: i32) {
        let _ = self.tx.send(Command::Tags(id, genres, tags, rating));
        self.requested = None;
    }
    pub fn export(&mut self, kind: Export, path: PathBuf, query: Query) {
        self.flush();
        let _ = self.tx.send(Command::Export(kind, path, query));
    }
}
impl Drop for Ledger {
    fn drop(&mut self) {
        self.finish("app_closed");
        let _ = self.tx.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(position: f64) -> Sample {
        Sample {
            position,
            duration: 200.0,
            speed: 1.0,
            volume: 70.0,
            ..Sample::default()
        }
    }
    #[test]
    fn only_audible_progress_counts() {
        assert_eq!(credit(s(10.0), s(11.0), 1.0), (1000.0, 1000.0));
        for b in [
            Sample {
                paused: true,
                ..s(11.0)
            },
            Sample {
                buffering: true,
                ..s(11.0)
            },
            Sample {
                muted: true,
                ..s(11.0)
            },
            Sample {
                idle: true,
                ..s(11.0)
            },
            Sample {
                seeking: true,
                ..s(11.0)
            },
            Sample {
                seek_serial: 1,
                ..s(11.0)
            },
            s(100.0),
            s(9.0),
            s(10.0),
        ] {
            assert_eq!(credit(s(10.0), b, 1.0), (0.0, 0.0));
        }
        assert_eq!(credit(s(10.0), s(70.0), 60.0), (0.0, 0.0));
        assert_eq!(credit(s(10.0), s(11.0), f64::NAN), (0.0, 0.0));
    }
    #[test]
    fn speed_changes_media_time_not_wall_time() {
        assert_eq!(
            credit(
                Sample {
                    speed: 2.0,
                    ..s(10.0)
                },
                Sample {
                    speed: 2.0,
                    ..s(12.0)
                },
                1.0
            ),
            (1000.0, 2000.0)
        );
    }
    #[test]
    fn repeated_sections_do_not_inflate_completion() {
        let mut c = Coverage::default();
        assert_eq!(c.add(0.0, 30.0), 30000.0);
        assert_eq!(c.add(0.0, 30.0), 30000.0);
        assert_eq!(c.add(20.0, 60.0), 60000.0);
        assert_eq!(c.add(100.0, 120.0), 80000.0);
    }
}
