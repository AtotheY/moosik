//! Whether the window's buffer swap waits for the screen.
//!
//! With vsync a swap waits for the compositor's frame callback, and on Wayland a compositor sends
//! none to a window it is not showing: a minimized one, or one on another desktop. The wait then
//! never ends and takes the UI thread with it, and everything the app does once a frame goes
//! with it: the queue stops at the end of a song, the media keys and the tray are not answered
//! and a quit does nothing until someone restores the window. winit cannot say a Wayland window
//! is minimized (`is_minimized` is `None` there, and egui-winit reads that as "not"), so eframe
//! never skips painting it, and the swap is where it hangs.
//!
//! So the wait is only wanted where it earns its keep, a window somebody is using: focused, or
//! with the pointer over it. Anywhere else the swap goes straight through, which costs one frame a
//! second while a song plays. Other platforms do not hang this way and are left as they are.
//!
//! A swap that does not wait also stops pacing anything that asks for a frame as soon as the last
//! one is out, a loading shimmer or a spinner: measured, such a window spent a whole core instead
//! of four percent. So where the swap does not wait, [`apply`] holds the frames to
//! [`FRAME`] apart itself.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long after the minimize button the window is taken to be gone, whether or not focus has
/// said so yet. Focus leaves within a few milliseconds of a real minimize; a compositor that
/// refuses one leaves the window as it was, and after this the wait is back.
const MINIMIZE_GRACE: Duration = Duration::from_millis(1500);

/// The least time between two frames of a window whose swap does not wait for the screen: sixty a
/// second, what a screen would have allowed.
const FRAME: Duration = Duration::from_millis(16);

/// When the minimize button last asked.
static MINIMIZED_AT: Mutex<Option<Instant>> = Mutex::new(None);

/// When the last frame of a window that does not wait was let through.
static LAST_FRAME: Mutex<Option<Instant>> = Mutex::new(None);

/// The window was just asked to minimize. The next swap must not wait: focus has not left yet, and
/// a swap that waits on a window that is about to go would be the one that hangs.
pub fn minimize_requested() {
    if let Ok(mut at) = MINIMIZED_AT.lock() {
        *at = Some(Instant::now());
    }
}

fn minimizing(now: Instant) -> bool {
    MINIMIZED_AT
        .lock()
        .ok()
        .and_then(|at| *at)
        .is_some_and(|at| now.saturating_duration_since(at) < MINIMIZE_GRACE)
}

/// Whether a swap should wait for the screen. Pure, for the test.
///
/// `hidden` is the app's own hide to the tray, `minimizing` a minimize it has just asked for;
/// otherwise a window that has neither focus nor the pointer is one nobody is looking at.
fn wants_vsync(focused: bool, pointer_inside: bool, hidden: bool, minimizing: bool) -> bool {
    !hidden && !minimizing && (focused || pointer_inside)
}

/// How long to hold a frame that arrives `now`, the last one having left at `last`. Pure, for the
/// test.
fn hold_for(last: Option<Instant>, now: Instant) -> Duration {
    last.map_or(Duration::ZERO, |at| {
        FRAME.saturating_sub(now.saturating_duration_since(at))
    })
}

/// Hold the frame about to be drawn to the rate a screen would have set.
fn hold_to_frame_rate() {
    let wait = {
        let Ok(last) = LAST_FRAME.lock() else { return };
        hold_for(*last, Instant::now())
    };
    if !wait.is_zero() {
        std::thread::sleep(wait);
    }
    if let Ok(mut last) = LAST_FRAME.lock() {
        *last = Some(Instant::now());
    }
}

/// Set the swap's wait for the frame being drawn. Call from the UI, where the window's context is
/// current. Cheap: the answer is remembered, so only a change reaches the driver.
pub fn apply(focused: bool, pointer_inside: bool, hidden: bool) {
    // A test aid for debug builds: `MOOSIK_NO_PACING=1` leaves the swap as eframe set it, to see
    // what a minimized window does without this module.
    #[cfg(debug_assertions)]
    {
        static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *OFF.get_or_init(|| std::env::var_os("MOOSIK_NO_PACING").is_some()) {
            return;
        }
    }
    // A test aid for debug builds: `MOOSIK_FAKE_UNFOCUSED=1` takes the window for one nobody is
    // using.
    #[cfg(debug_assertions)]
    let (focused, pointer_inside) = if std::env::var_os("MOOSIK_FAKE_UNFOCUSED").is_some() {
        (false, false)
    } else {
        (focused, pointer_inside)
    };
    let wait = wants_vsync(focused, pointer_inside, hidden, minimizing(Instant::now()));
    set(wait);
    if !wait {
        hold_to_frame_rate();
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn set(wait: bool) {
    use std::sync::atomic::{AtomicU8, Ordering};
    // 0 not yet set, 1 waiting, 2 not waiting.
    static CURRENT: AtomicU8 = AtomicU8::new(0);
    let want = if wait { 1 } else { 2 };
    if CURRENT.swap(want, Ordering::Relaxed) == want {
        return;
    }
    let done = swap_interval(i32::from(wait));
    tracing::debug!(wait, done, "the swap's wait for the screen");
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn set(_wait: bool) {}

/// `eglSwapInterval` on the context current on this thread, through the libEGL that is already
/// loaded: nothing here links or loads one. Whether the call went through. A window on GLX has no
/// EGL loaded, and is left alone.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn swap_interval(interval: i32) -> bool {
    use std::ffi::{c_int, c_uint, c_void};
    type CurrentDisplay = unsafe extern "C" fn() -> *mut c_void;
    type SwapInterval = unsafe extern "C" fn(*mut c_void, c_int) -> c_uint;
    // SAFETY: both symbols are looked up by name in a library that is already loaded
    // (`RTLD_NOLOAD` never loads one), and are called with the signatures EGL gives them. The
    // handle is released again; the library stays, since the window's own context holds it.
    unsafe {
        let lib = libc::dlopen(c"libEGL.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD);
        if lib.is_null() {
            return false;
        }
        let current = libc::dlsym(lib, c"eglGetCurrentDisplay".as_ptr());
        let interval_fn = libc::dlsym(lib, c"eglSwapInterval".as_ptr());
        let done = if current.is_null() || interval_fn.is_null() {
            false
        } else {
            let current: CurrentDisplay = std::mem::transmute(current);
            let interval_fn: SwapInterval = std::mem::transmute(interval_fn);
            let display = current();
            !display.is_null() && interval_fn(display, interval) != 0
        };
        libc::dlclose(lib);
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window in use waits for the screen; one nobody is looking at does not.
    #[test]
    fn only_a_window_in_use_waits_for_the_screen() {
        assert!(
            wants_vsync(true, true, false, false),
            "focused, pointer over"
        );
        assert!(
            wants_vsync(true, false, false, false),
            "typing with the pointer elsewhere"
        );
        assert!(
            wants_vsync(false, true, false, false),
            "pointer over an unfocused window"
        );
        assert!(
            !wants_vsync(false, false, false, false),
            "minimized, covered, on another desktop"
        );
    }

    /// Hidden to the tray or just minimized, the swap goes through even while focus is still
    /// reported: it is the frame that has to get past the swap for focus to be read at all.
    #[test]
    fn a_window_on_its_way_out_does_not_wait() {
        assert!(!wants_vsync(true, true, true, false), "hidden to the tray");
        assert!(!wants_vsync(true, true, false, true), "just minimized");
    }

    #[test]
    fn a_minimize_is_taken_as_done_only_for_a_moment() {
        let start = Instant::now();
        *MINIMIZED_AT.lock().unwrap() = Some(start);
        assert!(minimizing(start));
        assert!(minimizing(
            start + MINIMIZE_GRACE - Duration::from_millis(1)
        ));
        assert!(!minimizing(start + MINIMIZE_GRACE));
        *MINIMIZED_AT.lock().unwrap() = None;
        assert!(!minimizing(start));
    }

    /// A frame that follows another too soon is held until a frame's length has passed since it,
    /// and one that comes after a pause, or the first, is not held at all.
    #[test]
    fn frames_without_a_screen_are_held_to_sixty_a_second() {
        let start = Instant::now();
        assert_eq!(hold_for(None, start), Duration::ZERO, "the first frame");
        assert_eq!(
            hold_for(Some(start), start),
            FRAME,
            "straight after another: the whole frame"
        );
        assert_eq!(
            hold_for(Some(start), start + Duration::from_millis(10)),
            FRAME - Duration::from_millis(10),
            "partway: what is left"
        );
        assert_eq!(
            hold_for(Some(start), start + FRAME),
            Duration::ZERO,
            "a frame later"
        );
        assert_eq!(
            hold_for(Some(start), start + Duration::from_secs(1)),
            Duration::ZERO,
            "a frame a second while a song plays is never held"
        );
        assert_eq!(
            hold_for(Some(start + Duration::from_secs(1)), start),
            FRAME,
            "a clock that went backwards holds one frame, not for ever"
        );
    }

    /// With no window there is no context: the call finds nothing current and says so, it does
    /// not crash.
    #[test]
    fn setting_the_interval_with_no_window_is_harmless() {
        set(false);
        set(true);
    }
}
