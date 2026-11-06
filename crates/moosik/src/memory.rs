//! Giving freed memory back to the system.
//!
//! Parsing a browse response builds a tree of `serde_json::Value`s, megabytes of small
//! allocations that are all freed a moment later. glibc keeps them: it grows a heap per thread
//! (each of the runtime's workers gets its own), and a heap's free memory is only returned from
//! its top, so a handful of long-lived allocations left among the rest hold the whole range.
//! `scripts/perf.sh` measured about 15 MB that way after a signed-in start, memory the app is not
//! using and the system cannot have. Two things claw it back: [`tune`] changes how glibc
//! decides, once at startup, and [`trim_after_parse`] asks it to hand back what is free right
//! after a big response has been turned into the app's own types.
//!
//! Only glibc has these controls. Everywhere else both are nothing.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Allocations from this size up get their own mapping, which the system takes back the moment
/// they are freed. Setting it also stops glibc raising the threshold as it sees big frees, which
/// is how a decoded cover or a response body used to end up on the heap for good.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const MMAP_THRESHOLD: i32 = 256 * 1024;
/// How much free memory the top of a heap may hold before it is returned.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const TRIM_THRESHOLD: i32 = 512 * 1024;
/// Heaps the process may have: two for the runtime's workers, and the threads that are left share.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const ARENA_MAX: i32 = 2;

/// The least time between two trims. A burst of results (a feed and its playlists arrive
/// together) is trimmed once, after the first; a trim walks every heap, so it is not free.
const TRIM_EVERY: Duration = Duration::from_secs(2);

/// Tell glibc how to treat memory: mapping big allocations separately, trimming a heap's top
/// sooner, and keeping to a few heaps. Call once, before other threads start.
pub fn tune() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `mallopt` takes plain integers and only changes allocator policy; it is called
    // before any other thread exists, as its documentation asks for these parameters.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, MMAP_THRESHOLD);
        libc::mallopt(libc::M_TRIM_THRESHOLD, TRIM_THRESHOLD);
        libc::mallopt(libc::M_ARENA_MAX, ARENA_MAX);
    }
}

/// Whether enough time has passed since the last trim for another. Pure, for the test.
fn due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|at| now.saturating_duration_since(at) >= TRIM_EVERY)
}

/// Hand back what glibc is holding that nothing uses, after a big response has been parsed into
/// the app's own types and the parse tree dropped. Rate-limited, and callable from any thread.
pub fn trim_after_parse() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let now = Instant::now();
    {
        let Ok(mut last) = LAST.lock() else { return };
        if !due(*last, now) {
            return;
        }
        *last = Some(now);
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `malloc_trim` takes no pointers and is safe to call from any thread.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first trim goes, and then none for two seconds, so a burst of results costs one.
    #[test]
    fn trims_are_spaced_out() {
        let t0 = Instant::now();
        assert!(due(None, t0), "the first is due");
        assert!(!due(Some(t0), t0 + Duration::from_millis(500)), "too soon");
        assert!(
            !due(Some(t0), t0 + Duration::from_millis(1999)),
            "still too soon"
        );
        assert!(due(Some(t0), t0 + Duration::from_secs(2)), "due again");
        // A clock that appears to run backwards is "too soon", not a panic.
        assert!(!due(Some(t0 + Duration::from_secs(5)), t0));
    }

    /// Tuning and trimming run on this machine without crashing, whatever the platform does with
    /// them; where glibc is not in use they are nothing at all.
    #[test]
    fn tuning_and_trimming_are_safe_to_call() {
        tune();
        trim_after_parse();
        trim_after_parse();
    }
}
