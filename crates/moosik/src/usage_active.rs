//! One anonymous report per active profile and UTC day, independent of listening-time sharing.
use anyhow::Result;
use rusqlite::{Connection, params};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

// Each process obtains the same token through the day primary key. No database
// transaction is held across HTTP. A duplicate in-flight request is safe at the collector.
async fn report_days<F: Future<Output = Result<()>>>(
    path: &Path,
    observed: u64,
    today: u64,
    mut upload: impl FnMut(String) -> F,
) -> Result<()> {
    std::fs::create_dir_all(path.parent().expect("profile directory"))?;
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(2))?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS activity (
        day INTEGER PRIMARY KEY, token TEXT NOT NULL, delivered INTEGER NOT NULL DEFAULT 0
    )",
    )?;
    let yesterday = today.saturating_sub(1);
    db.execute("DELETE FROM activity WHERE day < ?1", [yesterday])?;
    if (yesterday..=today).contains(&observed) {
        db.execute(
            "INSERT INTO activity(day, token) VALUES (?1, ?2) ON CONFLICT(day) DO NOTHING",
            params![observed, format!("{:032x}", rand::random::<u128>())],
        )?;
    }
    let pending = db.prepare("SELECT day, token FROM activity WHERE delivered=0 AND day BETWEEN ?1 AND ?2 ORDER BY day")?
        .query_map(params![yesterday,today], |row| Ok((row.get::<_,u64>(0)?, row.get::<_,String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut result = Ok(());
    for (day, token) in pending {
        anyhow::ensure!(
            token.len() == 32 && token.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid activity receipt"
        );
        match upload(format!("{day} {token}")).await {
            Ok(()) => {
                db.execute(
                    "UPDATE activity SET delivered=1 WHERE day=?1 AND token=?2",
                    params![day, token],
                )?;
            }
            Err(error) => result = Err(error),
        }
    }
    result
}

fn utc_day() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64 / 86_400
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
                .join("active-days.sqlite3"),
            observed: None,
            delivered: Arc::new(AtomicU64::new(u64::MAX)),
            retry_at: Instant::now(),
            job: None,
        })
    }

    pub fn poll(&mut self, runtime: &tokio::runtime::Handle, active: bool) {
        let now = Instant::now();
        if active {
            let today = utc_day();
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
        let today = utc_day();
        if today > day + 1 {
            self.observed = None;
            return;
        }
        self.retry_at = now + Duration::from_secs(300);
        let path = self.path.clone();
        let delivered = Arc::clone(&self.delivered);
        self.job = Some(runtime.spawn(async move {
            let sent = report_days(&path, day, today, |body| async move {
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
    async fn offline_restarts_retry_yesterday_and_today_and_expire_older_receipts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("days.db");
        let mut attempts = Vec::new();
        assert!(
            report_days(&path, 21_000, 21_000, |body| {
                attempts.push(body);
                async { anyhow::bail!("offline") }
            })
            .await
            .is_err()
        );
        for day in [21_001, 21_001, 21_002] {
            report_days(&path, day, day, |body| {
                attempts.push(body);
                async { Ok(()) }
            })
            .await
            .unwrap();
        }
        assert_eq!(attempts.len(), 4);
        assert_eq!(attempts[0], attempts[1]);
        assert_eq!(attempts[2].len(), 38);
        assert!(attempts[2].starts_with("21001 "));
        assert_ne!(&attempts[1][6..], &attempts[2][6..]);
        let db = Connection::open(path).unwrap();
        assert_eq!(
            db.query_row("SELECT min(day) FROM activity", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            21_001
        );
    }

    #[tokio::test]
    async fn overlapping_instances_and_a_lost_reply_share_the_same_token() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("days.db");
        let mut first = String::new();
        let mut second = String::new();
        let (a, b) = tokio::join!(
            report_days(&path, 21_000, 21_000, |body| {
                first = body;
                async {
                    tokio::task::yield_now().await;
                    anyhow::bail!("lost response")
                }
            }),
            report_days(&path, 21_000, 21_000, |body| {
                second = body;
                async { Ok(()) }
            })
        );
        assert!(a.is_err());
        b.unwrap();
        assert!(!first.is_empty());
        assert_eq!(first, second);
        report_days(&path, 21_000, 21_000, |_| async {
            panic!("already acknowledged")
        })
        .await
        .unwrap();
    }
}
