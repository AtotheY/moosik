//! One anonymous report per active profile and UTC day, independent of listening-time sharing.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
struct Receipt {
    day: u64,
    token: u128,
    delivered: bool,
}

impl Receipt {
    fn save(&self, path: &Path) -> Result<()> {
        crate::persist::write_atomic(path, &serde_json::to_vec(self)?)?;
        Ok(())
    }
}

async fn report_day<F: Future<Output = Result<()>>>(
    path: &Path,
    day: u64,
    upload: impl FnOnce(String) -> F,
) -> Result<()> {
    let saved = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Receipt>(&bytes)
            .ok()
            .filter(|r| r.day == day),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut receipt = match saved {
        Some(r) if r.delivered => return Ok(()),
        Some(r) => r,
        None => {
            std::fs::create_dir_all(path.parent().expect("profile directory"))?;
            let r = Receipt {
                day,
                token: rand::random(),
                delivered: false,
            };
            // A lost HTTP response or app restart must resend the same token.
            r.save(path)?;
            r
        }
    };
    upload(format!("{day} {:032x}", receipt.token)).await?;
    receipt.delivered = true;
    receipt.save(path)
}

pub(crate) struct ActiveCounter {
    path: PathBuf,
    observed: Option<u64>,
    delivered: Arc<AtomicU64>,
    retry_at: Instant,
    job: Option<tokio::task::JoinHandle<()>>,
}

impl ActiveCounter {
    pub fn new() -> Option<Self> {
        if std::env::var_os("MOOSIK_DISABLE_ACTIVE_COUNT").is_some() {
            return None;
        }
        Some(Self {
            path: crate::profile_dirs()?
                .data_local_dir()
                .join("active-day.json"),
            observed: None,
            delivered: Arc::new(AtomicU64::new(u64::MAX)),
            retry_at: Instant::now(),
            job: None,
        })
    }

    pub fn poll(&mut self, runtime: &tokio::runtime::Handle, active: bool) {
        let now = Instant::now();
        if active {
            let today = chrono::Utc::now().timestamp().max(0) as u64 / 86_400;
            if self.observed != Some(today) {
                self.observed = Some(today);
                self.retry_at = now;
            }
        }
        let Some(day) = self.observed else { return };
        if self.delivered.load(Ordering::Relaxed) == day
            || now < self.retry_at
            || self.job.as_ref().is_some_and(|job| !job.is_finished())
        {
            return;
        }
        // The collector accepts only today and yesterday. Don't keep retrying an idle session.
        if chrono::Utc::now().timestamp().max(0) as u64 / 86_400 > day + 1 {
            self.observed = None;
            return;
        }
        self.retry_at = now + Duration::from_secs(300);
        let path = self.path.clone();
        let delivered = Arc::clone(&self.delivered);
        self.job = Some(runtime.spawn(async move {
            let sent = report_day(&path, day, |body| async move {
                let client = crate::trusted_requests::client_builder()
                    .timeout(Duration::from_secs(8))
                    .build()?;
                let response = client
                    .post("https://usage.moosik.dev/v1/active")
                    .body(body)
                    .send()
                    .await?;
                anyhow::ensure!(response.status() == 204, "activity report declined");
                Ok(())
            })
            .await;
            if sent.is_ok() {
                delivered.store(day, Ordering::Relaxed);
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn retries_and_restarts_reuse_one_token_and_next_day_rotates_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("active.json");
        let mut bodies = Vec::new();
        assert!(
            report_day(&path, 21_000, |b| {
                bodies.push(b);
                async { anyhow::bail!("offline") }
            })
            .await
            .is_err()
        );
        for day in [21_000, 21_000, 21_001] {
            report_day(&path, day, |b| {
                bodies.push(b);
                async { Ok(()) }
            })
            .await
            .unwrap();
        }
        assert_eq!(bodies.len(), 3);
        assert_eq!(bodies[0], bodies[1]);
        assert_eq!(bodies[1].len(), 38);
        assert_ne!(&bodies[1][6..], &bodies[2][6..]);
    }
}
