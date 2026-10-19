//! Sign in through a browser window this app owns.
//!
//! # Why this exists
//!
//! On Windows there is no other way left. The browser import (`crate::browser_cookies`) shells
//! out to `yt-dlp`, which cannot read a modern Chromium cookie store: since Chromium 127 the
//! values are sealed with App-Bound Encryption and only the browser's own process can unwrap
//! them (`yt-dlp` #10927), and Brave keeps an exclusive lock on the database while it runs on
//! top of that. OAuth is not a fallback either — Google stopped accepting Bearer tokens on the
//! YouTube Music InnerTube surface (sigma67/ytmusicapi#676, yt-dlp/yt-dlp#11462), and the TV
//! client that still accepts them answers in a shape none of this app's parsers read.
//!
//! Asking a browser we control sidesteps both walls. On Windows, WebView2 is a real Edge browser
//! the OS already ships; on Linux the same window runs on WebKitGTK. Either way its own cookie
//! API hands back *decrypted* values, HttpOnly included, because the answer comes from inside the
//! web engine's process. The user sees an ordinary Google sign-in page; nothing is scraped,
//! pasted, or exported.
//!
//! # Why it runs in its own process
//!
//! The window needs an event loop of its own — tao's on Windows, GTK's on Linux — and neither
//! can live beside eframe's winit loop: on Windows creating one faults natively, an exception
//! out of a Win32 callback (`STATUS_FATAL_USER_CALLBACK_EXCEPTION`), not a Rust panic, and not
//! something the app can catch; on Linux GTK is never initialised in the app's own process at
//! all. Running the window as a fixed extension of this same binary is what a child process is
//! for: `--sign-in-window <file>` is that half, `capture` below spawns it and reads what it
//! wrote. This costs nothing visible — the binary is GUI-subsystem on Windows, so the child has
//! no console window — and it means a crash in the sign-in window cannot take the music down
//! with it.
//!
//! macOS uses the same child-process design with WKWebView. Three details differ from Windows:
//!
//! - The page is shown with a Safari user agent. A bare WKWebView sends no `Version/x Safari/x`
//!   token, and YouTube Music answers that with a "not optimised for your browser" dialog.
//! - The cookie jar is read whole and filtered by domain (`session_cookie`). wry's
//!   `cookies_for_url` on macOS only returns cookies whose domain exactly equals the page's host,
//!   and the session cookies are set on `.youtube.com`, so it never found them and the window
//!   waited forever.
//! - The web view uses a non-persistent store, because WKWebView ignores a data directory. Google's
//!   session therefore does not outlive the window, so a refresh asks for the password again,
//!   and `forget` has no profile folder to delete.
//!
//! Both desktop platforms get it: on Windows it is the only path left (the browser import cannot
//! read a modern Chromium store), and on Linux it is the one that needs no browser already signed
//! in and no `yt-dlp` cookie plumbing, since the window owns the session it creates. WebKitGTK
//! itself is not bundled into the AppImage; the host's own is used, which most desktops carry.

use std::path::{Path, PathBuf};
use std::time::Duration;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use tao::dpi::LogicalSize;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use tao::event::{Event, StartCause, WindowEvent};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use tao::event_loop::{ControlFlow, EventLoopBuilder};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use tao::platform::run_return::EventLoopExtRunReturn;
#[cfg(target_os = "windows")]
use tao::platform::windows::EventLoopBuilderExtWindows;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use tao::window::WindowBuilder;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use wry::WebContext;
use wry::WebViewBuilder;

/// The value stored as the session's `source` so a stale cookie can be refreshed by reopening
/// this same window — see `Api::refresh_from_source`.
pub const SOURCE: &str = "window";

/// The argument that puts the binary into sign-in-window mode, followed by the file to write the
/// captured cookie to.
pub const ARG: &str = "--sign-in-window";

/// The sign-in page, pointed back at YouTube Music when it finishes.
///
/// Signing in on `accounts.google.com` alone leaves the Google cookies but not the `.youtube.com`
/// session; the `continue` target is what makes the browser come back through YouTube and set
/// the cookies this app actually needs before the window is polled.
const SIGN_IN_URL: &str = "https://accounts.google.com/ServiceLogin?service=youtube\
&continue=https%3A%2F%2Fmusic.youtube.com%2F";

/// How often the cookie jar is looked at while the window is open. Cheap: the API returns the
/// store's contents without a network round trip, and a poll only ever runs while the window is.
const POLL: Duration = Duration::from_secs(1);

/// Set in the child's environment when the app's own theme is dark; see [`set_dark`].
#[cfg(target_os = "linux")]
const DARK_ENV: &str = "REFRYT_SIGN_IN_DARK";

/// Whether the app's theme is dark, kept current by [`crate::theme::apply`].
static DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Record the app's polarity, so the sign-in window opens in the same one.
///
/// On Linux WebKitGTK follows GTK's dark preference for the page's `prefers-color-scheme`, and a
/// Flatpak's GTK cannot see the desktop's own setting, so the page came up light there while it
/// was dark from the AppImage. Following the app instead makes both agree with what is on screen.
pub fn set_dark(dark: bool) {
    DARK.store(dark, std::sync::atomic::Ordering::Relaxed);
}

/// Open the sign-in window and return the session it captures.
///
/// Runs the window as a child of this same binary — see the module doc for why it cannot be a
/// thread here — and waits for it, so the cookie only comes back once the user is done (or the
/// window is closed, which is an error with the child's own explanation).
pub async fn capture() -> Result<String> {
    let jar = temp_cookie_path();
    // The file holds a live session: gone whether this returns, fails, or the future is dropped.
    let _guard = Cleanup(jar.clone());

    let exe = std::env::current_exe().context("could not find refryt's own executable")?;
    let mut command = tokio::process::Command::new(exe);
    command
        .arg(ARG)
        .arg(&jar)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        // Quitting the app must not leave the window behind.
        .kill_on_drop(true);
    // The window is a GUI-subsystem binary, so nothing would appear without this anyway; it is
    // here for the same reason `browser_cookies` sets it — a console must never flash.
    #[cfg(target_os = "windows")]
    command.creation_flags(0x0800_0000);
    // wry's Linux backend builds the WebKitGTK widget into an X11 window and rejects a Wayland
    // one (`UnsupportedWindowHandle`), so the child is pointed at XWayland — which every Wayland
    // desktop ships. `GDK_BACKEND` is what actually does it: with `WAYLAND_DISPLAY` merely
    // unset, GDK's Wayland backend still connects to the default `wayland-0` socket, which is
    // exactly the session we are trying to steer away from.
    #[cfg(target_os = "linux")]
    {
        // This one window renders in software. It is a login form: nothing here needs GPU
        // acceleration, while the GPU path is where it fails — WebKitGTK's DMA-BUF renderer
        // draws a blank page on the proprietary NVIDIA driver (its `gbm_bo_map` import fails;
        // WebKit bugs #262607, #180739), and accelerated compositing has produced blank or
        // crash-on-resize windows across other driver stacks too. Disabling both is the
        // workaround every WebKitGTK app converges on, at a cost this window cannot notice.
        // An explicit choice from the environment still wins.
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            command.env("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
            command.env("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
        // The NVIDIA driver's explicit-sync support trips compositors that enforce the
        // acquire-point rule: the surface commits without one and the connection is killed
        // ("Error 71 (Protocol error) dispatching to Wayland display", GDK's
        // `wp_linux_drm_syncobj_surface_v1`). The driver's own switch turns it off — the
        // workaround every GTK/WebKitGTK app converges on for NVIDIA on Wayland.
        if std::path::Path::new("/sys/module/nvidia").exists()
            && std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_none()
        {
            command.env("__NV_DISABLE_EXPLICIT_SYNC", "1");
        }
        if DARK.load(std::sync::atomic::Ordering::Relaxed) {
            command.env(DARK_ENV, "1");
        }
    }

    let output = command
        .output()
        .await
        .context("could not start the sign-in window")?;

    if output.status.success() {
        let cookie = std::fs::read_to_string(&jar)
            .context("the sign-in window said it succeeded but left no session behind")?;
        let cookie = cookie.trim().to_owned();
        if cookie.is_empty() {
            bail!("the sign-in window returned an empty session");
        }
        return Ok(cookie);
    }

    // A crash leaves no reason of ours behind, only whatever GTK or WebKit last warned about
    // ("Can't connect to a11y bus" and the like), which would read as the cause.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if output.status.signal().is_some() {
            bail!("the sign-in window crashed. Try again");
        }
    }
    // Otherwise the child's stderr is its explanation — the last line it printed, which is the
    // specific reason rather than a protocol trace. Library warnings (GLib's `** (…): WARNING **`
    // form) are skipped for the same reason as above.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let why = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("** ("))
        .unwrap_or_default();
    if why.is_empty() {
        bail!("the sign-in window exited unexpectedly ({})", output.status);
    }
    bail!("{why}")
}

/// Forget the Google session the sign-in window accumulated.
///
/// Signing out has to reach this, or the next Log in finds Google still signed in inside the
/// window's own profile and silently re-captures the same account: no account to choose and no
/// way to sign in as anyone else. Best effort — the profile is only a cache of credentials, and
/// a failed delete just means the old behaviour.
pub fn forget() {
    if let Ok(dir) = profile_dir() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The child half: open the window, and on success write the cookie to `path`.
///
/// Called from `main` when [`ARG`] is present. A failure is printed by `main` and exits non-zero,
/// which is what tells [`capture`] the missing file is a failed sign-in rather than a bug.
pub fn capture_here(path: &Path) -> Result<()> {
    let cookie = run_window()?;
    std::fs::write(path, cookie)
        .with_context(|| format!("could not write the session to {}", path.display()))
}

/// Where the sign-in window keeps its WebView2 profile.
///
/// Under refryt's config directory so Google's own session outlives the process: reopening the
/// window later — including for a refresh — usually captures a fresh cookie without the user
/// typing anything.
fn profile_dir() -> Result<PathBuf> {
    let path = crate::auth::cookie_path()?;
    let parent = path
        .parent()
        .context("the cookie path has no directory to put the profile in")?;
    Ok(parent.join("sign-in-window"))
}

/// The window and its whole event loop. Blocks until the session is captured or the window is
/// closed, and only returns a cookie once `auth::validate_cookie`'s names are all present.
///
/// Windows and macOS: tao's loop with wry's WebView2 / WKWebView — see the module doc for why
/// the window cannot share the app's own loop. On macOS the child process owns its main thread,
/// which AppKit insists the event loop run on.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn run_window() -> Result<String> {
    #[cfg(target_os = "windows")]
    let dir = {
        let dir = profile_dir()?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        dir
    };

    // This process is the one that exists for this window, so the loop may as well own its
    // thread — but tao still wants the opt-in when the loop is not on the process's first.
    #[allow(unused_mut)]
    let mut builder = EventLoopBuilder::new();
    #[cfg(target_os = "windows")]
    builder.with_any_thread(true);
    let mut event_loop = builder.build();
    let window = WindowBuilder::new()
        .with_title("Sign in to YouTube Music")
        .with_inner_size(LogicalSize::new(560.0, 780.0))
        .with_min_inner_size(LogicalSize::new(420.0, 560.0))
        .build(&event_loop)
        .context("could not create the sign-in window")?;

    #[cfg(target_os = "windows")]
    let webview = {
        let mut context = WebContext::new(Some(dir));
        WebViewBuilder::new_with_web_context(&mut context)
            .with_url(SIGN_IN_URL)
            .build(&window)
            .context(
                "could not open the sign-in window. It needs the Microsoft Edge WebView2 \
                 runtime, which Windows 10 and 11 ship; if it was removed, install the \
                 Evergreen runtime from Microsoft and try again",
            )?
    };
    // WKWebView ignores a data directory, so the profile cannot live under the config folder
    // where `forget` could delete it. A non-persistent store makes signing out forget the Google
    // session by construction, at the price of typing the password again on a refresh.
    #[cfg(target_os = "macos")]
    let webview = WebViewBuilder::new()
        .with_incognito(true)
        // A bare WKWebView sends no `Version/x Safari/x` token, and YouTube Music answers that
        // with its "not optimised for your browser" dialog over the page. Say Safari, as
        // Safari does (the macOS token is frozen at 10_15_7 on purpose).
        .with_user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
        )
        .with_url(SIGN_IN_URL)
        .build(&window)
        .context("could not open the sign-in window")?;

    let mut outcome: Option<Result<String>> = None;
    let mut deadline = Instant::now() + POLL;
    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(deadline);
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                match session_cookie(&webview) {
                    Ok(Some(cookie)) => {
                        outcome = Some(Ok(cookie));
                        *control_flow = ControlFlow::Exit;
                    }
                    Ok(None) => deadline = Instant::now() + POLL,
                    Err(e) => {
                        outcome = Some(Err(e));
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                outcome = Some(Err(anyhow!(
                    "the sign-in window was closed before it finished signing in"
                )));
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });

    outcome.unwrap_or_else(|| bail!("the sign-in window closed without a result"))
}

/// Linux: a plain GTK window with the WebKit widget inside it, driven by GTK's own main loop.
///
/// Deliberately not tao plus wry's `build`: that path wraps the tao window as a *foreign* X11
/// window — X11-only, so a Wayland handle is rejected outright, and on some driver stacks the
/// widget never paints, leaving an empty window. `build_gtk` embeds the widget in an ordinary
/// GTK window instead, which is the path wry recommends on Linux and the one that behaves on
/// Wayland and X11 alike.
#[cfg(target_os = "linux")]
fn run_window() -> Result<String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gtk::prelude::*;
    use wry::WebViewBuilderExtUnix;

    let dir = profile_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;

    gtk::init().context("could not start GTK")?;
    if std::env::var_os(DARK_ENV).is_some()
        && let Some(settings) = gtk::Settings::default()
    {
        settings.set_gtk_application_prefer_dark_theme(true);
    }
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Sign in to YouTube Music");
    window.set_default_size(560, 780);
    window.set_size_request(420, 560);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.add(&vbox);

    let mut context = WebContext::new(Some(dir));
    let webview = Rc::new(
        WebViewBuilder::new_with_web_context(&mut context)
            .with_url(SIGN_IN_URL)
            .build_gtk(&vbox)
            .context(
                "could not open the sign-in window. It needs WebKitGTK (webkit2gtk-4.1) and \
                 GTK 3, which most desktops already carry; install them and try again, or use \
                 the browser import below",
            )?,
    );
    window.show_all();

    let outcome: Rc<RefCell<Option<Result<String>>>> = Rc::new(RefCell::new(None));
    {
        let outcome = Rc::clone(&outcome);
        window.connect_delete_event(move |_, _| {
            *outcome.borrow_mut() = Some(Err(anyhow!(
                "the sign-in window was closed before it finished signing in"
            )));
            gtk::main_quit();
            glib::Propagation::Proceed
        });
    }
    {
        let outcome = Rc::clone(&outcome);
        let webview = Rc::clone(&webview);
        glib::timeout_add_local(POLL, move || match session_cookie(&webview) {
            Ok(Some(cookie)) => {
                *outcome.borrow_mut() = Some(Ok(cookie));
                gtk::main_quit();
                glib::ControlFlow::Break
            }
            Ok(None) => glib::ControlFlow::Continue,
            Err(e) => {
                *outcome.borrow_mut() = Some(Err(e));
                gtk::main_quit();
                glib::ControlFlow::Break
            }
        });
    }
    gtk::main();

    outcome
        .borrow_mut()
        .take()
        .unwrap_or_else(|| bail!("the sign-in window closed without a result"))
}

/// The `Cookie` header for the session the window currently holds, if it is signed in.
///
/// A signed-out visitor to `music.youtube.com` still gets cookies, but not `SAPISID` — the name
/// `auth::validate_cookie` treats as the marker of a real session. Waiting for those names here
/// keeps a half-finished login from being handed to `install_cookie` as if it were one.
fn session_cookie(webview: &wry::WebView) -> Result<Option<String>> {
    #[cfg(not(target_os = "macos"))]
    let cookies = webview
        .cookies_for_url("https://music.youtube.com")
        .context("could not read the sign-in window's cookies")?;
    // wry's WKWebView `cookies_for_url` keeps only cookies whose domain *equals* the URL's host,
    // and the session cookies live on `.youtube.com`, so it would never return one. Read the
    // whole jar and keep what a request to music.youtube.com would send.
    #[cfg(target_os = "macos")]
    let cookies: Vec<_> = webview
        .cookies()
        .context("could not read the sign-in window's cookies")?
        .into_iter()
        .filter(|c| {
            let domain = c.domain().unwrap_or_default().trim_start_matches('.');
            domain == "youtube.com" || domain == "music.youtube.com"
        })
        .collect();

    let named = |name: &str| cookies.iter().any(|c| c.name() == name);
    if !named("SAPISID") || !(named("__Secure-1PSID") || named("__Secure-3PSID")) {
        return Ok(None);
    }

    let header = cookies
        .iter()
        .filter(|c| !c.name().is_empty())
        .map(|c| format!("{}={}", c.name(), c.value()))
        .collect::<Vec<_>>()
        .join("; ");
    Ok(Some(header))
}

/// Where the child writes the session for the parent to read. Unique per run, so two windows
/// opened back to back cannot read each other's file.
fn temp_cookie_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "refryt-sign-in-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ))
}

/// Delete the session file as soon as we are done with it: it is a credential.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
