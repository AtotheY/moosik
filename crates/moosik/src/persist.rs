//! Atomic replacement for local settings, other JSON stores, and per-user launcher artwork.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes concurrent writers to the same path: with a fixed temp name two saves (the
/// periodic playback write, eframe's own save on its timer and at exit, two downloads finishing
/// together) can open, truncate and interleave the same temp file before either rename — the
/// exact corruption this helper exists to prevent.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` to `path` atomically: to a sibling temp file, then renamed into place. The
/// rename is all-or-nothing on the same filesystem, so a crash or power loss mid-write can never
/// leave `path` holding a truncated, unparsable file — the previous version survives instead.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(e) = std::fs::write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

pub use write_atomic as write_json_atomic;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_file_and_leaves_no_temp_behind() {
        let dir = std::env::temp_dir().join(format!(
            "moosik-persist-test-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("could not create the test directory");
        let path = dir.join("store.json");

        write_json_atomic(&path, b"{\"a\":1}").expect("first write failed");
        write_json_atomic(&path, b"{\"a\":2}").expect("second write failed");
        assert_eq!(std::fs::read(&path).expect("read failed"), b"{\"a\":2}");

        let files: Vec<String> = std::fs::read_dir(&dir)
            .expect("could not list the test directory")
            .map(|e| {
                e.expect("bad entry")
                    .file_name()
                    .into_string()
                    .expect("bad name")
            })
            .collect();
        assert_eq!(files, ["store.json"], "a temp file was left behind");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
