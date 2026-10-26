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
//! - WKWebView ignores a data directory, so the profile is a named WebKit data store instead
//!   (macOS 14 and later). Its id lives in `sign-in-window.id` next to the session; `forget`
//!   deletes that file, and the next window starts a new store and removes the old one. Before
//!   macOS 14 there are no named stores, so the window runs incognito and a refresh cannot
//!   happen without the user signing in again.
//!
//! # Refreshing in the background
//!
//! Google's cookies go stale on their own, and `Api::refresh_from_source` renews them by running
//! this window again with `--silent`: hidden, pointed at YouTube Music rather than the login
//! page, and given a few seconds after the page loads for Google to renew the session. If the
//! profile is no longer signed in either, it gives up instead of showing a login page nobody
//! asked for. A window only ever appears because the user clicked Sign in.
//!
//! Both desktop platforms get it: on Windows it is the only path left (the browser import cannot
//! read a modern Chromium store), and on Linux it is the one that needs no browser already signed
//! in and no `yt-dlp` cookie plumbing, since the window owns the session it creates. WebKitGTK
//! itself is not bundled into the AppImage; the host's own is used, which most desktops carry.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
use wry::{PageLoadEvent, WebViewBuilder};

/// The value stored as the session's `source` so a stale cookie can be refreshed by reopening
/// this same window — see `Api::refresh_from_source`.
pub const SOURCE: &str = "window";

/// The argument that puts the binary into sign-in-window mode, followed by the file to write the
/// captured cookie to.
pub const ARG: &str = "--sign-in-window";

/// Follows [`ARG`]'s file to make the child a background refresh: no window, no login page.
pub const SILENT_ARG: &str = "--silent";

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

/// Where a background refresh points the window. YouTube Music itself, not `SIGN_IN_URL`: with
/// Google still signed in the profile, loading it is what renews the session; without, it would
/// be a login page in a window nobody can see.
const REFRESH_URL: &str = "https://music.youtube.com/";

/// How long a background refresh waits after the last page load before reading the jar. The
/// cookies on disk are the stale ones until the page has been answered and Google's own script
/// has renewed the short-lived `__Secure-1PSIDTS`, so the first read cannot be the moment the
/// page finishes. Counted from the *last* load because the page can move on by itself: a
/// signed-out profile ends on Google's consent or login page, which settles to "not signed in"
/// within seconds rather than at [`REFRESH_TIMEOUT`].
const SETTLE: Duration = Duration::from_secs(3);

/// The longest a background refresh may take, page load included, before it gives up.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(20);

/// Why a background refresh came back empty.
const NOT_REMEMBERED: &str = "the saved sign-in has ended; sign in again";

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
    run_child(false).await
}

/// Renew a stale session from the window's profile without showing anything.
///
/// Fails at once when there is no profile to renew from — never signed in through the window,
/// signed out since, or macOS before 14 — and otherwise when Google no longer knows the profile
/// either. Either way the caller tells the user to sign in again; this never opens a login page
/// by itself.
pub async fn refresh() -> Result<String> {
    if !remembers() {
        bail!(NOT_REMEMBERED);
    }
    run_child(true).await
}

/// Start the window as a child of this binary and wait for the session it writes.
async fn run_child(silent: bool) -> Result<String> {
    let jar = temp_cookie_path();
    // The file holds a live session: gone whether this returns, fails, or the future is dropped.
    let _guard = Cleanup(jar.clone());

    let exe = std::env::current_exe().context("could not find refryt's own executable")?;
    let mut command = tokio::process::Command::new(exe);
    command.arg(ARG).arg(&jar);
    if silent {
        command.arg(SILENT_ARG);
    }
    command
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
///
/// On macOS the profile is a WebKit data store, which only a process with WebKit running can
/// delete. Removing its id is what counts: the next window starts a new store, and removes the
/// old one as it opens (see `macos_store::remove_others`).
pub fn forget() {
    #[cfg(not(target_os = "macos"))]
    if let Ok(dir) = profile_dir() {
        let _ = std::fs::remove_dir_all(dir);
    }
    #[cfg(target_os = "macos")]
    if let Ok(path) = macos_store::id_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Whether the window has a profile a background refresh could renew the session from.
fn remembers() -> bool {
    #[cfg(not(target_os = "macos"))]
    return profile_dir().is_ok_and(|dir| dir.is_dir());
    #[cfg(target_os = "macos")]
    return macos_store::available() && matches!(macos_store::id(false), Ok(Some(_)));
}

/// The child half: open the window, and on success write the cookie to `path`.
///
/// Called from `main` when [`ARG`] is present, with `silent` when [`SILENT_ARG`] follows. A
/// failure is printed by `main` and exits non-zero, which is what tells [`capture`] the missing
/// file is a failed sign-in rather than a bug.
pub fn capture_here(path: &Path, silent: bool) -> Result<()> {
    let cookie = run_window(silent)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .context("could not create the private sign-in handoff")?;
    std::io::Write::write_all(&mut file, cookie.as_bytes())
        .context("could not write the sign-in handoff")
}

/// A file or folder next to the stored session, in refryt's config directory.
fn beside_session(name: &str) -> Result<PathBuf> {
    let path = crate::auth::cookie_path()?;
    let parent = path
        .parent()
        .context("the cookie path has no directory to put the profile in")?;
    Ok(parent.join(name))
}

/// Where the sign-in window keeps its WebView2 / WebKitGTK profile.
///
/// Under refryt's config directory so Google's own session outlives the process: reopening the
/// window later — including for a refresh — usually captures a fresh cookie without the user
/// typing anything.
#[cfg(not(target_os = "macos"))]
fn profile_dir() -> Result<PathBuf> {
    beside_session("sign-in-window")
}

/// The macOS profile: a WebKit data store named by a random id kept beside the session.
///
/// WKWebView takes no data directory, so a named store (macOS 14+) is the only way for Google's
/// session to outlive the window and still be something `forget` can drop.
#[cfg(target_os = "macos")]
mod macos_store {
    use std::io::Read;
    use std::path::PathBuf;

    use anyhow::{Context, Result};

    pub fn id_path() -> Result<PathBuf> {
        super::beside_session("sign-in-window.id")
    }

    /// Whether this macOS has named data stores. They arrived in macOS 14; refryt runs from 11.
    pub fn available() -> bool {
        use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};
        NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
            majorVersion: 14,
            minorVersion: 0,
            patchVersion: 0,
        })
    }

    /// The store's id, made and saved on first use when `create` is set.
    pub fn id(create: bool) -> Result<Option<[u8; 16]>> {
        let path = id_path()?;
        if let Some(id) = std::fs::read_to_string(&path).ok().and_then(|s| parse(&s)) {
            return Ok(Some(id));
        }
        if !create {
            return Ok(None);
        }
        let mut id = [0u8; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut id))
            .context("could not make an id for the sign-in window's profile")?;
        std::fs::write(&path, format(&id))
            .with_context(|| format!("could not write {}", path.display()))?;
        Ok(Some(id))
    }

    /// Delete every store this app owns except `keep`: what is left after a sign-out.
    ///
    /// Must run on the main thread with the event loop going, since WebKit answers through
    /// completion handlers. Best effort; a store that cannot be removed now is tried next time.
    pub fn remove_others(keep: [u8; 16]) {
        use wry::{WebView, WebViewExtDarwin};
        let _ = WebView::fetch_data_store_identifiers(move |ids| {
            for id in ids.into_iter().filter(|id| *id != keep) {
                WebView::remove_data_store(&id, |_| {});
            }
        });
    }

    pub fn parse(text: &str) -> Option<[u8; 16]> {
        let text = text.trim();
        if text.len() != 32 {
            return None;
        }
        let mut id = [0u8; 16];
        for (i, byte) in id.iter_mut().enumerate() {
            *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(id)
    }

    pub fn format(id: &[u8; 16]) -> String {
        id.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// What one look at the window found: [`poll`]'s answer to both event loops.
enum Poll {
    Done(Result<String>),
    Wait,
}

/// One look at the window. A sign-in waits for as long as the user takes. A background refresh
/// waits only until no page has loaded for [`SETTLE`], then takes what is there, and never runs past
/// [`REFRESH_TIMEOUT`].
fn poll(webview: &wry::WebView, silent: bool, started: Instant, loaded: Option<Instant>) -> Poll {
    if silent {
        let now = Instant::now();
        if !loaded.is_some_and(|at| now >= at + SETTLE) {
            return if now >= started + REFRESH_TIMEOUT {
                Poll::Done(Err(anyhow!(NOT_REMEMBERED)))
            } else {
                Poll::Wait
            };
        }
    }
    match session_cookie(webview) {
        Ok(Some(cookie)) => Poll::Done(Ok(cookie)),
        // Loaded, settled, and still signed out: Google's own session in the profile has ended.
        Ok(None) if silent => Poll::Done(Err(anyhow!(NOT_REMEMBERED))),
        Ok(None) => Poll::Wait,
        Err(e) => Poll::Done(Err(e)),
    }
}

/// The window and its whole event loop. Blocks until the session is captured or the window is
/// closed, and only returns a cookie once `auth::validate_cookie`'s names are all present.
///
/// Windows and macOS: tao's loop with wry's WebView2 / WKWebView — see the module doc for why
/// the window cannot share the app's own loop. On macOS the child process owns its main thread,
/// which AppKit insists the event loop run on.
///
/// `silent` is a background refresh: the window is never shown (and on macOS the child takes no
/// Dock icon), and the page is YouTube Music rather than the login page.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn run_window(silent: bool) -> Result<String> {
    use std::cell::Cell;
    use std::rc::Rc;

    #[cfg(target_os = "windows")]
    let dir = {
        let dir = profile_dir()?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        dir
    };
    // The store is chosen before anything opens. A sign-in makes one if there is none yet (the
    // first sign-in, or the first after a sign-out); a refresh only runs when one exists.
    #[cfg(target_os = "macos")]
    let store = if macos_store::available() {
        macos_store::id(true)?
    } else {
        None
    };

    // This process is the one that exists for this window, so the loop may as well own its
    // thread — but tao still wants the opt-in when the loop is not on the process's first.
    #[allow(unused_mut)]
    let mut builder = EventLoopBuilder::new();
    #[cfg(target_os = "windows")]
    builder.with_any_thread(true);
    #[allow(unused_mut)]
    let mut event_loop = builder.build();
    // A Dock icon appearing and vanishing at launch would be the one visible trace of a refresh.
    #[cfg(target_os = "macos")]
    if silent {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Prohibited);
    }
    let window = WindowBuilder::new()
        .with_title("Sign in to YouTube Music")
        .with_inner_size(LogicalSize::new(560.0, 780.0))
        .with_min_inner_size(LogicalSize::new(420.0, 560.0))
        .with_visible(!silent)
        .build(&event_loop)
        .context("could not create the sign-in window")?;

    let url = if silent { REFRESH_URL } else { SIGN_IN_URL };
    let loaded: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let on_load = {
        let loaded = Rc::clone(&loaded);
        move |event: PageLoadEvent, _url: String| {
            if matches!(event, PageLoadEvent::Finished) {
                loaded.set(Some(Instant::now()));
            }
        }
    };

    #[cfg(target_os = "windows")]
    let webview = {
        let mut context = WebContext::new(Some(dir));
        WebViewBuilder::new_with_web_context(&mut context)
            .with_url(url)
            .with_on_page_load_handler(on_load)
            .build(&window)
            .context(
                "could not open the sign-in window. It needs the Microsoft Edge WebView2 \
                 runtime, which Windows 10 and 11 ship; if it was removed, install the \
                 Evergreen runtime from Microsoft and try again",
            )?
    };
    // WKWebView ignores a data directory, so the profile is a named data store (macOS 14+) that
    // `forget` drops by forgetting its id. Older macOS has no named stores, and there the window
    // stays non-persistent: signing out still forgets Google's session, at the price of
    // `refresh` never being able to renew one.
    #[cfg(target_os = "macos")]
    let webview = {
        use wry::WebViewBuilderExtDarwin;
        let builder = match store {
            Some(id) => WebViewBuilder::new().with_data_store_identifier(id),
            None => WebViewBuilder::new().with_incognito(true),
        };
        builder
            // A bare WKWebView sends no `Version/x Safari/x` token, and YouTube Music answers
            // that with its "not optimised for your browser" dialog over the page. Say Safari,
            // as Safari does (the macOS token is frozen at 10_15_7 on purpose).
            .with_user_agent(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
            )
            .with_url(url)
            .with_on_page_load_handler(on_load)
            .build(&window)
            .context("could not open the sign-in window")?
    };

    let started = Instant::now();
    let mut outcome: Option<Result<String>> = None;
    let mut deadline = started + POLL;
    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(deadline);
        match event {
            // The stores a sign-out left behind go now, while WebKit is running to delete them.
            #[cfg(target_os = "macos")]
            Event::NewEvents(StartCause::Init) => {
                if let Some(id) = store {
                    macos_store::remove_others(id);
                }
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                match poll(&webview, silent, started, loaded.get()) {
                    Poll::Done(result) => {
                        outcome = Some(result);
                        *control_flow = ControlFlow::Exit;
                    }
                    Poll::Wait => deadline = Instant::now() + POLL,
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
///
/// `silent` builds into a `gtk::OffscreenWindow`: WebKitGTK loads and renders there as it would
/// on screen, but nothing is ever mapped.
#[cfg(target_os = "linux")]
fn run_window(silent: bool) -> Result<String> {
    use std::cell::{Cell, RefCell};
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
    let window: gtk::Window = if silent {
        gtk::OffscreenWindow::new().upcast()
    } else {
        gtk::Window::new(gtk::WindowType::Toplevel)
    };
    window.set_title("Sign in to YouTube Music");
    window.set_default_size(560, 780);
    window.set_size_request(420, 560);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.add(&vbox);

    let loaded: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let on_load = {
        let loaded = Rc::clone(&loaded);
        move |event: PageLoadEvent, _url: String| {
            if matches!(event, PageLoadEvent::Finished) {
                loaded.set(Some(Instant::now()));
            }
        }
    };
    let mut context = WebContext::new(Some(dir));
    let webview = Rc::new(
        WebViewBuilder::new_with_web_context(&mut context)
            .with_url(if silent { REFRESH_URL } else { SIGN_IN_URL })
            .with_on_page_load_handler(on_load)
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
        let started = Instant::now();
        glib::timeout_add_local(POLL, move || {
            match poll(&webview, silent, started, loaded.get()) {
                Poll::Done(result) => {
                    *outcome.borrow_mut() = Some(result);
                    gtk::main_quit();
                    glib::ControlFlow::Break
                }
                Poll::Wait => glib::ControlFlow::Continue,
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

// Only the macOS store has logic of its own to test; the window itself needs a display.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn the_store_id_round_trips_through_its_file_text() {
        let id: [u8; 16] = core::array::from_fn(|i| (i as u8).wrapping_mul(37));
        let text = macos_store::format(&id);
        assert_eq!(text.len(), 32);
        assert_eq!(macos_store::parse(&format!("{text}\n")), Some(id));
    }

    #[test]
    fn a_damaged_store_id_reads_as_none() {
        assert_eq!(macos_store::parse(""), None);
        assert_eq!(macos_store::parse("abc"), None);
        assert_eq!(macos_store::parse(&"zz".repeat(16)), None);
        assert_eq!(macos_store::parse(&"ab".repeat(17)), None);
    }
}
