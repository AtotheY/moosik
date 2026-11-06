//! Crash log.
//!
//! Release builds are `panic = "abort"` and stripped, so a panic vanishes and the process dies.
//! A hook still runs before the abort, so this writes one report to the config dir and the next
//! launch offers it — copy the details, or open a prefilled issue. Nothing is sent anywhere on
//! its own.

use std::path::PathBuf;

/// A crash recorded by a previous run.
#[derive(Clone, Debug)]
pub struct Crash {
    pub message: String,
    pub location: String,
    pub time_unix: u64,
    pub version: String,
    pub backtrace: String,
}

/// Where the last crash is kept — the same config dir as `settings.json`, so it travels with the
/// app's own data and needs no new directory.
fn path() -> Option<PathBuf> {
    let dirs = crate::profile_dirs()?;
    Some(dirs.config_dir().join("crash.json"))
}

/// Install the panic hook. Call once, after tracing is up and before the window opens. Under
/// `panic = "abort"` the hook still runs; only unwinding (`catch_unwind`) is unavailable.
pub fn install_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = payload_message(info);
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_owned();
        // `force_capture`, not `capture`: a user's build usually has `RUST_BACKTRACE` unset, and
        // an empty report is worse than a stripped one.
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        tracing::error!(%message, %location, "panic");

        let report = serde_json::json!({
            "time": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            "version": env!("CARGO_PKG_VERSION"),
            "message": message,
            "location": location,
            "thread": thread,
            "backtrace": backtrace,
        });
        let Some(path) = path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&report) {
            let _ = crate::persist::write_json_atomic(&path, &bytes);
        }
    }));
}

/// The crash the previous run left behind, if there is one. Malformed or missing files read as
/// "no crash" rather than an error — this is best-effort.
pub fn last() -> Option<Crash> {
    let bytes = std::fs::read(path()?).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(Crash {
        message: v.get("message")?.as_str()?.to_owned(),
        location: v
            .get("location")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_owned(),
        time_unix: v.get("time").and_then(|x| x.as_u64()).unwrap_or(0),
        version: v
            .get("version")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_owned(),
        backtrace: v
            .get("backtrace")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_owned(),
    })
}

/// Remove the stored crash — called once the report has been shown.
pub fn clear() {
    if let Some(path) = path() {
        let _ = std::fs::remove_file(path);
    }
}

fn payload_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_owned()
    }
}
