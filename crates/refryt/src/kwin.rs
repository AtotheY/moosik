//! KDE Plasma on Wayland: hide the app's own window to the tray and bring it back, through KWin.
//!
//! Wayland has no hidden window. winit's `set_visible(false)` is ignored there, so "hide to
//! tray" is a minimise (see `App::hide_command`), which leaves the window in the taskbar, the
//! pager and Alt+Tab — not the tray-only state it is on Windows. And Wayland gives a client no
//! way to undo a minimise itself: winit's `set_minimized(false)` is ignored too ("Unminimizing is
//! ignored on Wayland"), and an activation request from a window nobody is looking at only makes
//! its taskbar entry ask for attention. So a tray click hid the window and could never show it
//! again; the taskbar entry was the only way back.
//!
//! The proper route back is the activation token Plasma hands a tray item when it is clicked,
//! which neither `tray-icon` nor `ksni` passes on. Until one of them does, KWin's own scripting
//! interface on the session bus does both halves: a short script finds this process's window,
//! and on hide takes it off the taskbar, the pager and the window switcher as it minimises it;
//! on show it puts it back on all three, restores it and makes it the active window. It runs
//! only on Plasma, and only on this process's own window.
//!
//! Best-effort throughout: off Plasma, from a sandbox without access to KWin's bus name, or with
//! KWin's scripting disabled, the call fails quietly and the window is left minimised, as it was
//! before.

use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};

use anyhow::{Context, Result};

/// Whether this session is KDE Plasma's, where KWin's scripting interface exists.
pub fn is_plasma() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|desktop| desktop.split(':').any(|d| d.eq_ignore_ascii_case("KDE")))
}

/// The script KWin runs on this process's window: hide it to the tray, or bring it back and
/// activate it. `windowList` and `activeWindow` are Plasma 6's names; `clientList` and
/// `activeClient` are Plasma 5's.
fn script(pid: u32, hidden: bool) -> String {
    let activate = if hidden {
        ""
    } else {
        "if (workspace.windowList) workspace.activeWindow = w; else workspace.activeClient = w;"
    };
    format!(
        "const all = workspace.windowList ? workspace.windowList() : workspace.clientList();\n\
         for (const w of all) {{\n\
         \x20   if (w.pid === {pid} && w.normalWindow) {{\n\
         \x20       w.skipTaskbar = {hidden};\n\
         \x20       w.skipPager = {hidden};\n\
         \x20       w.skipSwitcher = {hidden};\n\
         \x20       w.minimized = {hidden};\n\
         \x20       {activate}\n\
         \x20   }}\n\
         }}\n"
    )
}

/// Hide this process's window to the tray (`true`), or bring it back and focus it (`false`).
/// Returns at once: the calls go to one worker thread in order, so a quick hide and show cannot
/// land the wrong way round. A failure is logged at debug level only, since off Plasma it is
/// expected.
pub fn set_hidden(hidden: bool) {
    static WORKER: OnceLock<Sender<bool>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = channel::<bool>();
        std::thread::spawn(move || {
            while let Ok(hidden) = rx.recv() {
                if let Err(e) = run_script(hidden) {
                    tracing::debug!(error = %format!("{e:#}"), hidden, "kwin: no window change");
                }
            }
        });
        tx
    });
    let _ = worker.send(hidden);
}

fn run_script(hidden: bool) -> Result<()> {
    let pid = std::process::id();
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("refryt-kwin-{pid}.js"));
    std::fs::write(&path, script(pid, hidden)).context("could not write the KWin script")?;
    let result = load_and_run(&path, &format!("refryt-window-{pid}"));
    let _ = std::fs::remove_file(&path);
    result
}

fn load_and_run(path: &std::path::Path, name: &str) -> Result<()> {
    const KWIN: Option<&str> = Some("org.kde.KWin");
    const SCRIPTING: Option<&str> = Some("org.kde.kwin.Scripting");
    let bus = zbus::blocking::Connection::session().context("no session bus")?;
    // A leftover from an earlier call that never got unloaded would make the load fail.
    let _ = bus.call_method(KWIN, "/Scripting", SCRIPTING, "unloadScript", &(name,));
    let id: i32 = bus
        .call_method(
            KWIN,
            "/Scripting",
            SCRIPTING,
            "loadScript",
            &(path.to_string_lossy().as_ref(), name),
        )
        .context("KWin's scripting is not on the session bus")?
        .body()
        .deserialize()
        .context("KWin's loadScript answered something unexpected")?;
    if id < 0 {
        anyhow::bail!("KWin refused the script");
    }
    // Plasma 6 names the script's object `/Scripting/Script<id>`; Plasma 5 used `/<id>`.
    let run = |object: String| {
        bus.call_method(
            KWIN,
            object.as_str(),
            Some("org.kde.kwin.Script"),
            "run",
            &(),
        )
    };
    let ran = run(format!("/Scripting/Script{id}")).or_else(|_| run(format!("/{id}")));
    // The script has done its work once `run` returns; unload it either way.
    let _ = bus.call_method(KWIN, "/Scripting", SCRIPTING, "unloadScript", &(name,));
    ran.context("KWin did not run the script")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_targets_this_pid_only() {
        assert!(script(4242, true).contains("w.pid === 4242"));
    }

    #[test]
    fn hiding_leaves_the_taskbar_and_showing_comes_back_focused() {
        let hide = script(1, true);
        assert!(hide.contains("w.skipTaskbar = true"));
        assert!(hide.contains("w.minimized = true"));
        assert!(!hide.contains("activeWindow = w"));
        let show = script(1, false);
        assert!(show.contains("w.skipTaskbar = false"));
        assert!(show.contains("w.minimized = false"));
        assert!(show.contains("activeWindow = w"));
    }
}
