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
        self.credit_at(ms, clock_ms(), self.consent_since.elapsed());
    }

    fn credit_at(&mut self, ms: u64, now: u64, since_consent: Duration) {
        // Clamp a sample that straddles consent or UTC midnight.
        let ms = ms.min(since_consent.as_millis() as u64).min(now % DAY);
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
        self.poll_at(now, clock_ms() / DAY, |body, tx| {
            runtime.spawn(async move {
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
            })
        });
    }

    fn poll_at(
        &mut self,
        now: Instant,
        today: u64,
        upload: impl FnOnce(String, mpsc::Sender<bool>) -> JoinHandle<()>,
    ) {
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
        let job = upload(body, tx);
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
        reporter.credit_at(65_000, day * DAY + 120_000, Duration::from_secs(65));
        let token = reporter.buckets[0].token.clone();
        drop(reporter);
        let reopened = Reporter::open_at(path.clone()).unwrap();
        assert_eq!(reopened.buckets[0].token, token);
        assert_eq!(reopened.buckets[0].ms / MINUTE, 1);
        drop(reopened);
        std::fs::write(
            &path,
            format!(r#"[{{"day":{day},"token":"unrelated private text","ms":65000}}]"#),
        )
        .unwrap();
        assert!(Reporter::open_at(path).unwrap().buckets.is_empty());
    }

    #[test]
    fn samples_straddling_consent_and_midnight_only_credit_the_allowed_interval() {
        let dir = tempfile::tempdir().unwrap();
        let mut reporter = Reporter::open_at(dir.path().join("usage.json")).unwrap();
        let day = 21_000;
        reporter.credit_at(1000, day * DAY + 10_000, Duration::ZERO);
        assert!(reporter.buckets.is_empty());
        reporter.credit_at(1000, day * DAY + 10_000, Duration::from_millis(250));
        reporter.credit_at(1000, (day + 1) * DAY + 100, Duration::from_secs(60));
        assert_eq!(reporter.buckets.len(), 2);
        assert_eq!(
            (reporter.buckets[0].day, reporter.buckets[0].ms),
            (day, 250)
        );
        assert_eq!(
            (reporter.buckets[1].day, reporter.buckets[1].ms),
            (day + 1, 100)
        );
        assert_ne!(reporter.buckets[0].token, reporter.buckets[1].token);
    }

    #[tokio::test]
    async fn reporting_waits_for_a_durable_minute_and_retries_the_latest_total() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        let mut reporter = Reporter::open_at(path.clone()).unwrap();
        let start = Instant::now();
        let day = 21_000;
        let no_upload = |_, _| panic!("unexpected upload");
        reporter.credit_at(59_000, day * DAY + 59_000, Duration::from_secs(59));
        reporter.poll_at(start, day, no_upload);
        // A directory in place of the state file makes the atomic rename fail.
        std::fs::create_dir(&path).unwrap();
        reporter.credit_at(1000, day * DAY + MINUTE, Duration::from_secs(60));
        reporter.poll_at(start, day, no_upload);
        std::fs::remove_dir(&path).unwrap();
        let token = reporter.buckets[0].token.clone();
        reporter.poll_at(start, day, |body, result| {
            let saved: Vec<Bucket> =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(saved[0].token, token);
            assert_eq!(saved[0].ms, MINUTE);
            assert_eq!(body, format!("{day} {token} 1"));
            result.send(false).unwrap();
            tokio::spawn(async {})
        });
        reporter.credit_at(MINUTE, day * DAY + 2 * MINUTE, Duration::from_secs(120));
        reporter.poll_at(start + Duration::from_secs(299), day, no_upload);
        reporter.poll_at(start + Duration::from_secs(300), day, |body, result| {
            assert_eq!(body, format!("{day} {token} 2"));
            result.send(true).unwrap();
            tokio::spawn(async {})
        });
        reporter.poll_at(start + Duration::from_secs(600), day, no_upload);
        assert_eq!(reporter.buckets[0].acknowledged, 2);
    }
}
