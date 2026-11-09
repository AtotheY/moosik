//! Optional aggregate usage. This module receives only newly credited wall-clock milliseconds.
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;

const MINUTE: u64 = 60_000;
const DAY: u64 = 1_440 * MINUTE;
const ENDPOINT: &str = "https://usage.moosik.dev/v1/minutes";

#[derive(Serialize, Deserialize)]
struct Bucket {
    day: u64, // UTC days since Unix epoch
    token: String,
    ms: u64,
    #[serde(skip)]
    acknowledged: u64,
}

impl Bucket {
    fn new(day: u64) -> Self {
        Self {
            day,
            token: format!("{:032x}", rand::random::<u128>()),
            ms: 0,
            acknowledged: 0,
        }
    }
}

fn clock_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

pub(crate) struct Reporter {
    path: PathBuf,
    buckets: Vec<Bucket>,
    next_attempt: Instant,
    last_tick: Option<Instant>,
    consent_since: Instant,
    dirty: bool,
    in_flight: Option<(JoinHandle<()>, mpsc::Receiver<bool>, u64, u64)>,
}

impl Reporter {
    fn path() -> Option<PathBuf> {
        Some(
            crate::profile_dirs()?
                .data_local_dir()
                .join("usage-minutes.json"),
        )
    }

    pub fn open() -> Option<Self> {
        Self::open_at(Self::path()?)
    }

    fn open_at(path: PathBuf) -> Option<Self> {
        std::fs::create_dir_all(path.parent()?).ok()?;
        let mut buckets: Vec<Bucket> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).ok()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return None,
        };
        // Never forward arbitrary strings from a corrupted or hand-edited local file.
        buckets.retain(|b| {
            b.ms <= DAY
                && b.token.len() == 32
                && b.token
                    .bytes()
                    .all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch))
        });
        Some(Self {
            path,
            buckets,
            next_attempt: Instant::now(),
            consent_since: Instant::now(),
            last_tick: None,
            dirty: false,
            in_flight: None,
        })
    }

    pub fn forget() {
        if let Some(path) = Self::path() {
            let _ = std::fs::remove_file(path);
        }
    }

    pub fn credit(&mut self, ms: u64) {
        // Samples straddling the opt-in or midnight boundary cannot charge pre-consent time.
        let now = clock_ms();
        let ms = ms
            .min(self.consent_since.elapsed().as_millis() as u64)
            .min(now % DAY);
        if ms == 0 {
            return;
        }
        let today = now / DAY;
        if !self.buckets.iter().any(|b| b.day == today) {
            self.buckets.push(Bucket::new(today));
        }
        let b = self.buckets.iter_mut().find(|b| b.day == today).unwrap();
        let before = b.ms / MINUTE;
        b.ms = (b.ms.saturating_add(ms)).min(DAY);
        self.dirty |= before != b.ms / MINUTE;
    }

    fn save(&self) -> bool {
        serde_json::to_vec(&self.buckets)
            .is_ok_and(|v| crate::persist::write_atomic(&self.path, &v).is_ok())
    }

    pub fn poll(&mut self, runtime: &tokio::runtime::Handle) {
        let now = Instant::now();
        if self
            .last_tick
            .is_some_and(|t| now.duration_since(t) < Duration::from_secs(1))
        {
            return;
        }
        self.last_tick = Some(now);
        let today = clock_ms() / DAY;
        let len = self.buckets.len();
        self.buckets
            .retain(|b| b.day <= today && today - b.day <= 1);
        self.dirty |= len != self.buckets.len();
        if self.dirty {
            if !self.save() {
                return;
            }
            self.dirty = false;
        }
        if let Some((_, rx, day, minutes)) = &self.in_flight {
            match rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => return,
                Ok(true) => {
                    if let Some(b) = self.buckets.iter_mut().find(|b| b.day == *day) {
                        b.acknowledged = *minutes;
                    }
                }
                _ => {}
            }
            self.in_flight = None;
        }
        if now < self.next_attempt {
            return;
        }
        let Some(b) = self.buckets.iter().find(|b| b.ms / MINUTE > b.acknowledged) else {
            return;
        };
        let minutes = b.ms / MINUTE;
        let body = format!("{} {} {}", b.day, b.token, minutes);
        let (tx, rx) = mpsc::channel();
        let job = runtime.spawn(async move {
            let ok = async {
                let client = crate::trusted_requests::client_builder()
                    .timeout(Duration::from_secs(8))
                    .build()?;
                Ok::<_, reqwest::Error>(
                    client
                        .post(ENDPOINT)
                        .body(body)
                        .send()
                        .await?
                        .status()
                        .as_u16()
                        == 204,
                )
            }
            .await
            .unwrap_or(false);
            let _ = tx.send(ok);
        });
        self.in_flight = Some((job, rx, b.day, minutes));
        self.next_attempt = now + Duration::from_secs(300);
    }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        if let Some((job, ..)) = self.in_flight.take() {
            job.abort();
        }
        if !self.buckets.is_empty() {
            self.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_starts_off_and_restarts_reuse_the_daily_bucket() {
        assert!(
            !serde_json::from_str::<crate::settings::Settings>("{}")
                .unwrap()
                .share_listening_minutes
        );
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("usage.json");
        let day = clock_ms() / DAY;
        let mut reporter = Reporter::open_at(path.clone()).unwrap();
        assert!(reporter.buckets.is_empty());
        reporter.credit(0);
        assert!(!path.exists());
        let mut b = Bucket::new(day);
        let token = b.token.clone();
        b.ms = 65_000;
        reporter.buckets.push(b);
        drop(reporter);
        let reopened = Reporter::open_at(path.clone()).unwrap();
        assert_eq!(reopened.buckets[0].token, token);
        assert_eq!(reopened.buckets[0].ms / MINUTE, 1);
        assert_ne!(Bucket::new(day + 1).token, token);
        drop(reopened);
        std::fs::write(
            &path,
            format!(r#"[{{"day":{day},"token":"unrelated private text","ms":65000}}]"#),
        )
        .unwrap();
        assert!(Reporter::open_at(path).unwrap().buckets.is_empty());
    }
}
