//! Window fullscreen, read and set in one place.
//!
//! Everywhere but macOS this is egui's own `ViewportCommand::Fullscreen` and the fullscreen flag
//! winit reports. macOS needs something else. Native fullscreen there moves the window into a
//! Space of its own, with a black backdrop and nothing of the desktop behind it, so a translucent
//! window shows black instead of the blurred desktop. macOS therefore uses "simple" fullscreen:
//! the window stays on the current Space, grows to cover its screen, and the Dock and menu bar
//! are hidden while it does. This is the same thing winit's `set_simple_fullscreen` and many
//! terminals do.
//!
//! Every caller goes through [`is`] and [`set`] rather than the viewport directly, so the
//! fullscreen player, the video view and the startup check all agree on what "fullscreen" means.

/// Whether the window is fullscreen right now.
pub fn is(ctx: &egui::Context) -> bool {
    let native = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
    #[cfg(target_os = "macos")]
    {
        native || macos::active()
    }
    #[cfg(not(target_os = "macos"))]
    {
        native
    }
}

/// Take the window into or out of fullscreen.
pub fn set(ctx: &egui::Context, on: bool) {
    #[cfg(target_os = "macos")]
    {
        // A window that is natively fullscreen anyway (restored that way by the system) is
        // taken out the native way first; simple fullscreen then takes over from there.
        if !on && ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        macos::set(on);
        ctx.request_repaint();
    }
    #[cfg(not(target_os = "macos"))]
    {
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::Mutex;

    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationPresentationOptions, NSWindow};
    use objc2_foundation::NSRect;

    /// What to put back when fullscreen ends: the window's frame and the app's presentation
    /// options (the Dock and menu bar behaviour) from before. `Some` while fullscreen.
    struct Saved {
        frame: NSRect,
        options: NSApplicationPresentationOptions,
    }

    // Only plain values are stored here. The window itself is looked up again on every call,
    // because AppKit objects cannot be kept in a static shared across threads.
    static SAVED: Mutex<Option<Saved>> = Mutex::new(None);

    pub fn active() -> bool {
        SAVED.lock().map(|s| s.is_some()).unwrap_or(false)
    }

    /// The app's main window: the first normal-level one. The tray's status item and menus
    /// live on higher levels and are not ours to resize.
    fn main_window(app: &NSApplication) -> Option<objc2::rc::Retained<NSWindow>> {
        app.windows().iter().find(|w| w.level() == 0)
    }

    pub fn set(on: bool) {
        // AppKit may only be touched from the main thread, which is where egui's UI runs on
        // macOS. Anywhere else, do nothing rather than risk it.
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let Some(window) = main_window(&app) else {
            return;
        };
        let Ok(mut saved) = SAVED.lock() else {
            return;
        };
        match (on, saved.is_some()) {
            (true, false) => {
                let Some(screen) = window.screen() else {
                    return;
                };
                *saved = Some(Saved {
                    frame: window.frame(),
                    options: app.presentationOptions(),
                });
                app.setPresentationOptions(
                    NSApplicationPresentationOptions::HideDock
                        | NSApplicationPresentationOptions::HideMenuBar,
                );
                window.setFrame_display(screen.frame(), true);
            }
            (false, true) => {
                if let Some(previous) = saved.take() {
                    app.setPresentationOptions(previous.options);
                    window.setFrame_display(previous.frame, true);
                }
            }
            // Already in the state asked for.
            _ => {}
        }
    }
}
