//! Blur behind the window.
//!
//! KWin does not blur a window that has not asked, which is why a transparent winit window stays
//! merely transparent on its own however see-through it is — the compositor wants its own opt-in.
//! One real backend, the natural way for its platform:
//!
//! - **KDE Wayland**: two protocols for one job. The standard
//!   `ext_background_effect_manager_v1` first — what KWin 6.7 offers after it dropped its own
//!   `org_kde_kwin_blur` outright (no deprecation period; the global is simply gone), and what
//!   niri and Mutter 51 speak — falling back to `org_kde_kwin_blur` on Plasma 6.6 and earlier,
//!   where the standard one does not exist yet. Either way a region is load-bearing: `wl_region`
//!   has no notion of rounded corners, so one is computed by hand (`rounded_region`) or the
//!   compositor blurs a rectangle behind a window that isn't one. The standard protocol also
//!   carries a `capabilities` event, so the Settings toggle follows the Blur desktop effect
//!   switch live instead of offering blur the compositor has been told not to do.
//!
//! - **Windows**: DWM's own acrylic, `DWMWA_SYSTEMBACKDROP_TYPE` (Windows 11 22H2 and later),
//!   see [`windows`]. No region either: DWM follows the window's shape.
//!
//! macOS asks the window server directly (`CGSSetWindowBackgroundBlurRadius`), see [`macos`].
//! The blur follows the window's shape, which is why the macOS window is a native one that the
//! system rounds (`main.rs`) rather than a borderless rectangle: with a borderless window the
//! blur would show as a square behind the app's rounded corners.
//!
//! Silent by design. GNOME before 51, wlroots, X11, Windows 10: all take the same
//! unavailable path — the window stays merely translucent, and Settings explains that rather
//! than offering a switch that would do nothing.

/// The blur region for a rounded window, as rectangles.
///
/// `wl_region` is unions of rectangles and nothing else, so the corner arc is a staircase: one
/// full-width band for the straight middle, then a row per pixel of radius at each end, inset
/// by the circle's own `r - sqrt(r² - dy²)`. Blurring the corners square would put a blurred
/// rectangle behind a rounded window, which the desktop shows as four grey nicks.
///
/// Pure, so the geometry is testable without a compositor.
pub fn rounded_region(w: u32, h: u32, radius: u32) -> Vec<(i32, i32, i32, i32)> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let r = radius.min(w / 2).min(h / 2);
    if r == 0 {
        return vec![(0, 0, w as i32, h as i32)];
    }
    let mut rects = Vec::with_capacity(2 * r as usize + 1);
    // The straight middle, full width — only where the two arcs have not already met, which
    // happens once a window's height drops to twice its (clamped) radius.
    if h > 2 * r {
        rects.push((0, r as i32, w as i32, (h - 2 * r) as i32));
    }
    // One row per pixel of the arc, top and bottom, inset to follow the circle.
    for dy in 0..r {
        let y = (r - dy) as f32 - 0.5;
        let inset = (r as f32 - ((r as f32).powi(2) - y * y).max(0.0).sqrt()).round() as u32;
        let inset = inset.min(w / 2);
        let width = w.saturating_sub(2 * inset);
        if width == 0 {
            continue;
        }
        rects.push((inset as i32, dy as i32, width as i32, 1));
        rects.push((inset as i32, (h - dy - 1) as i32, width as i32, 1));
    }
    rects
}

#[cfg(target_os = "linux")]
pub use wayland::Blur;

#[cfg(target_os = "macos")]
pub use macos::Blur;

#[cfg(target_os = "windows")]
pub use windows::Blur;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub use stub::Blur;

/// macOS: the window server's own background blur, `CGSSetWindowBackgroundBlurRadius` — what
/// Terminal, iTerm2, WezTerm and Alacritty use for a blurred translucent window. It blurs
/// whatever is behind the window wherever the window is not opaque, so it needs nothing from how
/// the window is drawn.
///
/// It is a private (but long-stable) CoreGraphics call. A native `NSVisualEffectView` placed
/// behind the app's own view was tried first and never drew anything under the Metal layer.
#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// How strongly to blur. Matches what terminals use for a frosted look.
    const RADIUS: i32 = 30;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGSMainConnectionID() -> u32;
        fn CGSSetWindowBackgroundBlurRadius(connection: u32, window: u32, radius: i32) -> i32;
    }

    #[derive(Default)]
    pub struct Blur {
        content: Option<Retained<NSView>>,
        applied: Option<bool>,
    }

    impl Blur {
        pub fn new(handles: &(impl HasWindowHandle + raw_window_handle::HasDisplayHandle)) -> Self {
            let content = handles.window_handle().ok().and_then(|h| match h.as_raw() {
                // SAFETY: eframe hands out the window's live content `NSView`, on the main
                // thread, while the window exists.
                RawWindowHandle::AppKit(handle) => unsafe {
                    Retained::retain(handle.ns_view.as_ptr().cast::<NSView>())
                },
                _ => None,
            });
            Self {
                content,
                applied: None,
            }
        }

        pub fn available(&self) -> bool {
            self.content.is_some()
        }

        /// Cheap to call every frame: the window server is only asked when `on` changed. The
        /// size and corner radius are ignored, because the blur follows the window's own shape.
        pub fn set(&mut self, on: bool, _size: (u32, u32), _radius: u32) {
            if self.applied == Some(on) {
                return;
            }
            let Some(window) = self.content.as_ref().and_then(|c| c.window()) else {
                return;
            };
            let number = window.windowNumber();
            if number <= 0 {
                return;
            }
            // SAFETY: plain calls into CoreGraphics with this process's own connection and one
            // of its window numbers; a failure just means no blur.
            let status = unsafe {
                CGSSetWindowBackgroundBlurRadius(
                    CGSMainConnectionID(),
                    number as u32,
                    if on { RADIUS } else { 0 },
                )
            };
            if status != 0 {
                tracing::warn!(status, "could not set the window's background blur");
            }
            self.applied = Some(on);
        }

        /// The window server's blur takes no tint from the window: nothing to tell it.
        pub fn set_dark(&mut self, _dark: bool) {}
    }
}

/// Everywhere that is not Linux, macOS or Windows — whatever else this ever compiles for: the
/// same shape, doing nothing.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod stub {
    #[derive(Default)]
    pub struct Blur;

    impl Blur {
        pub fn new(
            _handles: &(impl raw_window_handle::HasWindowHandle + raw_window_handle::HasDisplayHandle),
        ) -> Self {
            Self
        }

        pub fn available(&self) -> bool {
            false
        }

        pub fn set(&mut self, _on: bool, _size: (u32, u32), _radius: u32) {}

        pub fn set_dark(&mut self, _dark: bool) {}
    }
}

/// Windows: DWM's acrylic, and the groundwork that lets the transparent shell show through at
/// all.
///
/// The shell is presented through a DirectComposition swapchain (`main.rs`), which DWM blends
/// with real per-pixel alpha, but it sits on top of the window's GDI redirection surface. Nothing
/// ever paints that surface, and unpainted it is opaque white, which is what made the
/// see-through shell read as a flat grey. So it is painted black — once now, and on every erase
/// and resize after — inside a frame extended over the client area: there black is "glass",
/// which shows the desktop, or the backdrop below when one is asked for. (The window cannot
/// simply be created without the surface: winit can ask for that, but eframe offers no way
/// through to it, and a CBT hook's style changes are ignored.)
///
/// The frame is extended from the bottom edge only. Extended from the top as well, DWM draws its
/// own caption buttons in it, under the app's.
///
/// DWM turns the backdrop flat grey whenever the window is inactive. While blur is on, every
/// deactivation is followed by a second, "active" `WM_NCACTIVATE` sent straight to the default
/// window procedure: winit has already seen the real one (it tracks focus by it), so only DWM's
/// idea of the window's state is touched.
#[cfg(target_os = "windows")]
mod windows {
    use std::sync::atomic::{AtomicBool, Ordering};

    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, S_OK, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        BLACK_BRUSH, FillRect, GetDC, GetStockObject, HDC, ReleaseDC,
    };
    use windows_sys::Win32::UI::Controls::MARGINS;
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, GetClientRect, IsWindowVisible, WM_ERASEBKGND, WM_NCACTIVATE, WM_SIZE,
    };

    /// Whether the backdrop is on, for the subclass procedure, which has no other state.
    static BACKDROP_ON: AtomicBool = AtomicBool::new(false);

    pub struct Blur {
        /// The window, once the groundwork above is laid. `None` without a Win32 handle.
        hwnd: Option<HWND>,
        /// Whether DWM accepted the backdrop when probed. What greys out the settings toggle.
        backdrop: bool,
        last: Option<bool>,
        last_dark: Option<bool>,
        /// Whether DWM has been told the window is active since the backdrop went on.
        activated: bool,
    }

    impl Blur {
        pub fn new(handles: &(impl HasWindowHandle + HasDisplayHandle)) -> Self {
            let hwnd = handles.window_handle().ok().and_then(|h| match h.as_raw() {
                RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
                _ => None,
            });
            let Some(h) = hwnd else {
                return Self {
                    hwnd: None,
                    backdrop: false,
                    last: None,
                    last_dark: None,
                    activated: false,
                };
            };
            // SAFETY: eframe's own live window, on the thread that owns it. The subclass lives
            // as long as the window and holds no data.
            let backdrop = unsafe {
                // Bottom only, but taller than any window: the glass still covers it all.
                let glass = MARGINS {
                    cxLeftWidth: 0,
                    cxRightWidth: 0,
                    cyTopHeight: 0,
                    cyBottomHeight: i16::MAX as i32,
                };
                DwmExtendFrameIntoClientArea(h, &glass);
                SetWindowSubclass(h, Some(subclass), 1, 0);
                paint_black(h);
                // The system's own rounded corners and shadow. DWM squares them again itself
                // when the window is maximized or snapped edge to edge.
                set_attribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND);
                // There is no query for "does this Windows offer the backdrop" short of asking
                // for it: probe with the real one and turn it straight back off. A refusal is a
                // Windows older than 11 22H2, where the attribute is unknown.
                let accepted = set_attribute(h, DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW);
                if accepted {
                    set_attribute(h, DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_NONE);
                }
                accepted
            };
            Self {
                hwnd,
                backdrop,
                last: None,
                last_dark: None,
                activated: false,
            }
        }

        pub fn available(&self) -> bool {
            self.backdrop
        }

        /// Cheap to call every frame: DWM is only asked when `on` changed. No region: DWM
        /// follows the window's own shape, so `size` and `radius` go unused here.
        pub fn set(&mut self, on: bool, _size: (u32, u32), _radius: u32) {
            if !self.backdrop {
                return;
            }
            let Some(h) = self.hwnd else { return };
            if self.last != Some(on) {
                let kind = if on {
                    DWMSBT_TRANSIENTWINDOW
                } else {
                    DWMSBT_NONE
                };
                BACKDROP_ON.store(on, Ordering::Relaxed);
                // SAFETY: the live window, on its own thread.
                unsafe { set_attribute(h, DWMWA_SYSTEMBACKDROP_TYPE, kind) };
                self.last = Some(on);
                self.activated = false;
            }
            // Turned on while the window is inactive — or before it is even shown, as on the
            // first frame — DWM keeps the backdrop flat until the next activation, so tell it
            // the window is active once there is a visible window to tell.
            // SAFETY: the live window, on its own thread.
            if on && !self.activated && unsafe { IsWindowVisible(h) } != 0 {
                unsafe { DefWindowProcW(h, WM_NCACTIVATE, 1, 0) };
                self.activated = true;
            }
        }

        /// Acrylic is tinted light unless the window says it is dark. Follows the palette, not
        /// the system theme, because a dark theme can be picked on a light desktop.
        pub fn set_dark(&mut self, dark: bool) {
            if self.last_dark == Some(dark) {
                return;
            }
            let Some(h) = self.hwnd else { return };
            // SAFETY: the live window, on its own thread.
            unsafe { set_attribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, i32::from(dark)) };
            self.last_dark = Some(dark);
        }
    }

    /// `true` when DWM took the value.
    unsafe fn set_attribute(h: HWND, attribute: i32, value: i32) -> bool {
        let hr = unsafe {
            DwmSetWindowAttribute(
                h,
                attribute as u32,
                (&raw const value).cast(),
                size_of::<i32>() as u32,
            )
        };
        hr == S_OK
    }

    unsafe fn fill_black(h: HWND, dc: HDC) {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe {
            GetClientRect(h, &mut rect);
            FillRect(dc, &rect, GetStockObject(BLACK_BRUSH));
        }
    }

    unsafe fn paint_black(h: HWND) {
        unsafe {
            let dc = GetDC(h);
            if dc != 0 {
                fill_black(h, dc);
                ReleaseDC(h, dc);
            }
        }
    }

    /// Keeps the redirection surface black — erased black instead of left to the (absent)
    /// class brush, and repainted after a resize grows it — and the backdrop live while the
    /// window is inactive.
    unsafe extern "system" fn subclass(
        h: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if msg == WM_ERASEBKGND {
            unsafe { fill_black(h, wparam as HDC) };
            return 1;
        }
        let result = unsafe { DefSubclassProc(h, msg, wparam, lparam) };
        match msg {
            WM_SIZE => unsafe { paint_black(h) },
            WM_NCACTIVATE if wparam == 0 && BACKDROP_ON.load(Ordering::Relaxed) => unsafe {
                DefWindowProcW(h, WM_NCACTIVATE, 1, lparam);
            },
            _ => {}
        }
        result
    }
}

#[cfg(target_os = "linux")]
mod wayland {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
    use wayland_client::backend::{Backend, ObjectId};
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{
        wl_compositor::WlCompositor, wl_region::WlRegion, wl_registry::WlRegistry,
        wl_surface::WlSurface,
    };
    use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
    use wayland_protocols::ext::background_effect::v1::client::{
        ext_background_effect_manager_v1::{self, Capability, ExtBackgroundEffectManagerV1},
        ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
    };
    use wayland_protocols_plasma::blur::client::{
        org_kde_kwin_blur::OrgKdeKwinBlur, org_kde_kwin_blur_manager::OrgKdeKwinBlurManager,
    };

    use super::rounded_region;

    /// The queue's user data. One real event rides through it — the standard protocol's
    /// `capabilities` — and the registry's globals are read once at startup. The queue exists so
    /// both have somewhere to go.
    struct State {
        /// Shared with `Inner`: whether the compositor can blur, as last announced.
        blur: Arc<AtomicBool>,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ExtBackgroundEffectManagerV1, ()> for State {
        fn event(
            state: &mut Self,
            _: &ExtBackgroundEffectManagerV1,
            event: <ExtBackgroundEffectManagerV1 as Proxy>::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            // Sent on bind and whenever the user flips the Blur desktop effect, so the Settings
            // toggle follows the effect switch live.
            if let ext_background_effect_manager_v1::Event::Capabilities { flags } = event {
                let blur = matches!(flags, WEnum::Value(c) if c.contains(Capability::Blur));
                state.blur.store(blur, Ordering::Relaxed);
            }
        }
    }

    wayland_client::delegate_noop!(State: ignore WlCompositor);
    wayland_client::delegate_noop!(State: ignore WlRegion);
    wayland_client::delegate_noop!(State: ignore WlSurface);
    wayland_client::delegate_noop!(State: ignore ExtBackgroundEffectSurfaceV1);
    wayland_client::delegate_noop!(State: ignore OrgKdeKwinBlurManager);
    wayland_client::delegate_noop!(State: ignore OrgKdeKwinBlur);

    /// Which protocol drives the blur. Both speak the same region language — surface-local
    /// rectangles, applied on the surface's next commit — so everything downstream of the bind
    /// is one match on this.
    enum Manager {
        /// The standard `ext-background-effect-v1`: KWin 6.7 (its own protocol is gone),
        /// niri, Mutter 51.
        Ext(ExtBackgroundEffectManagerV1),
        /// The KDE one: Plasma 6.6 and earlier, which is the only thing they advertise.
        Kde(OrgKdeKwinBlurManager),
    }

    /// The per-surface object each protocol creates. Both are kept for the window's life once
    /// created; "off" is never a release, because releasing and later touching the object is the
    /// protocol error that kills the whole connection.
    enum BlurSurface {
        /// Off is a NULL region, double-buffered like any other state.
        Ext(ExtBackgroundEffectSurfaceV1),
        /// Off is the KDE manager's `unset` on the surface.
        Kde(OrgKdeKwinBlur),
    }

    pub struct Blur {
        inner: Option<Inner>,
        /// What was last pushed: state, size and corner radius. An unchanged frame is a struct
        /// comparison and an early return, so this costs nothing per frame.
        last: Option<(bool, u32, u32, u32)>,
    }

    struct Inner {
        conn: Connection,
        queue: EventQueue<State>,
        qh: QueueHandle<State>,
        compositor: WlCompositor,
        surface: WlSurface,
        manager: Manager,
        blur: Option<BlurSurface>,
        /// Whether the compositor can blur, as the standard protocol's `capabilities` event last
        /// announced. The KDE protocol has no such event — offering it at all is the offer.
        blur_supported: Arc<AtomicBool>,
    }

    impl Blur {
        /// Connect once, from eframe's raw handles. Never fails loudly: everything that is not
        /// a Wayland session that offers one of the two blur protocols simply comes back
        /// unavailable.
        pub fn new(handles: &(impl HasWindowHandle + HasDisplayHandle)) -> Self {
            Self {
                inner: Inner::new(handles),
                last: None,
            }
        }

        /// Whether a protocol was bound and — on the standard one — the compositor can actually
        /// blur. What greys out the settings toggle.
        pub fn available(&self) -> bool {
            self.inner
                .as_ref()
                .is_some_and(|i| i.blur_supported.load(Ordering::Relaxed))
        }

        /// Push the region for this frame, if anything about it changed.
        pub fn set(&mut self, on: bool, size: (u32, u32), radius: u32) {
            let want = (on, size.0, size.1, radius);
            if self.last == Some(want) {
                return;
            }
            let Some(inner) = self.inner.as_mut() else {
                return;
            };
            inner.apply(on, size, radius);
            self.last = Some(want);
        }

        /// The compositor's blur takes no tint from the window: nothing to tell it.
        pub fn set_dark(&mut self, _dark: bool) {}
    }

    impl Inner {
        fn new(handles: &(impl HasWindowHandle + HasDisplayHandle)) -> Option<Self> {
            let RawDisplayHandle::Wayland(display) = handles.display_handle().ok()?.as_raw() else {
                return None;
            };
            let RawWindowHandle::Wayland(window) = handles.window_handle().ok()?.as_raw() else {
                return None;
            };

            // SAFETY: winit owns this `wl_display` for the life of the process, and a foreign
            // display is adopted as a guest — dropping our backend does not disconnect it.
            let backend = unsafe { Backend::from_foreign_display(display.display.as_ptr().cast()) };
            let conn = Connection::from_backend(backend);

            // This roundtrips, which is why it happens once at startup and never in a frame:
            // libwayland hands each event to its owning queue, so winit's are queued rather
            // than stolen, but a blocking read on the UI thread mid-frame would still be a
            // stall. After the setup below the module only ever writes.
            let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
            let qh = queue.handle();
            let compositor: WlCompositor = globals.bind(&qh, 1..=6, ()).ok()?;

            let blur_supported = Arc::new(AtomicBool::new(false));
            let mut state = State {
                blur: Arc::clone(&blur_supported),
            };

            // The standard protocol first, the KDE one only where the standard is absent. Each
            // has exactly one version, and asking for a range a compositor cannot satisfy is
            // itself a protocol error — hence `1..=1` on both.
            let manager = match globals.bind::<ExtBackgroundEffectManagerV1, _, _>(&qh, 1..=1, ()) {
                Ok(m) => Manager::Ext(m),
                Err(_) => {
                    let m: OrgKdeKwinBlurManager = globals.bind(&qh, 1..=1, ()).ok()?;
                    blur_supported.store(true, Ordering::Relaxed);
                    Manager::Kde(m)
                }
            };

            // The `capabilities` event is sent in response to the bind, so it rides behind it
            // in the queue. One blocking roundtrip — the second and last of the program, the
            // same shape as `registry_queue_init`'s — guarantees it has been read before the
            // first frame asks `available()`.
            let _ = queue.roundtrip(&mut state);

            // SAFETY: winit's own `wl_surface`, alive as long as the window. `from_ptr`
            // validates the interface against the proxy's class, so a mismatch is an error
            // rather than a malformed request.
            let id = unsafe {
                ObjectId::from_ptr(WlSurface::interface(), window.surface.as_ptr().cast())
            }
            .ok()?;
            let surface = WlSurface::from_id(&conn, id).ok()?;

            Some(Self {
                conn,
                queue,
                qh,
                compositor,
                surface,
                manager,
                blur: None,
                blur_supported,
            })
        }

        fn apply(&mut self, on: bool, size: (u32, u32), radius: u32) {
            // Non-blocking: drains what has already been delivered (a re-announced
            // `capabilities` among it), so our queue cannot grow without bound over a long
            // session. It never touches the socket.
            let mut state = State {
                blur: Arc::clone(&self.blur_supported),
            };
            let _ = self.queue.dispatch_pending(&mut state);

            if !on {
                if self.blur.is_some() {
                    match &self.manager {
                        Manager::Kde(m) => m.unset(&self.surface),
                        // The standard protocol has no manager-level unset: a NULL region is
                        // "off", double-buffered like any other state, so it lands on the same
                        // commit as the frame.
                        Manager::Ext(_) => {
                            if let Some(BlurSurface::Ext(s)) = self.blur.as_ref() {
                                s.set_blur_region(None);
                            }
                        }
                    }
                    let _ = self.conn.flush();
                }
                return;
            }

            if self.blur.is_none() {
                self.blur = Some(match &self.manager {
                    Manager::Ext(m) => {
                        BlurSurface::Ext(m.get_background_effect(&self.surface, &self.qh, ()))
                    }
                    Manager::Kde(m) => BlurSurface::Kde(m.create(&self.surface, &self.qh, ())),
                });
            }
            let Some(blur) = self.blur.as_ref() else {
                return;
            };
            let region = self.compositor.create_region(&self.qh, ());
            for (x, y, w, h) in rounded_region(size.0, size.1, radius) {
                region.add(x, y, w, h);
            }
            match blur {
                // Same region language both ways: surface-local coordinates, double-buffered,
                // applied on the surface's next commit — the frame eframe is about to present.
                // Only the KDE object carries a `commit` request of its own.
                BlurSurface::Ext(s) => s.set_blur_region(Some(&region)),
                BlurSurface::Kde(b) => {
                    b.set_region(Some(&region));
                    b.commit();
                }
            }
            region.destroy();
            // Nothing else flushes our requests.
            let _ = self.conn.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rounded_region;

    /// The region must stay inside the window, or KWin blurs a band of the desktop beside it.
    #[test]
    fn the_region_stays_inside_the_window() {
        for (w, h, r) in [(1100, 720, 10), (400, 300, 10), (64, 48, 10), (900, 600, 0)] {
            for (x, y, rw, rh) in rounded_region(w, h, r) {
                assert!(x >= 0 && y >= 0, "{x},{y} for {w}x{h} r{r}");
                assert!(rw > 0 && rh > 0, "empty rect for {w}x{h} r{r}");
                assert!(
                    x + rw <= w as i32 && y + rh <= h as i32,
                    "{x},{y} {rw}x{rh} escapes {w}x{h}"
                );
            }
        }
    }

    /// A square window is one rectangle — the maximised and `--decorated` cases, where a staircase
    /// of rectangles would add cost for no visible gain.
    #[test]
    fn no_radius_is_a_single_rect() {
        assert_eq!(rounded_region(800, 600, 0), vec![(0, 0, 800, 600)]);
    }

    /// The corners are the only thing given up: everything but four `r × r` squares is blurred.
    #[test]
    fn only_the_corners_are_left_out() {
        let (w, h, r) = (1100u32, 720u32, 10u32);
        let covered: i64 = rounded_region(w, h, r)
            .iter()
            .map(|(_, _, rw, rh)| *rw as i64 * *rh as i64)
            .sum();
        let total = w as i64 * h as i64;
        assert!(covered <= total, "covered {covered} of {total}");
        assert!(
            covered >= total - 4 * (r as i64).pow(2),
            "covered {covered}, too little of {total}"
        );
    }

    /// A window smaller than twice the radius clamps instead of producing negative widths.
    #[test]
    fn a_tiny_window_does_not_go_negative() {
        for (w, h) in [(1, 1), (4, 4), (10, 6), (0, 0)] {
            for (_, _, rw, rh) in rounded_region(w, h, 10) {
                assert!(rw > 0 && rh > 0, "{w}x{h} produced {rw}x{rh}");
            }
        }
    }
}
