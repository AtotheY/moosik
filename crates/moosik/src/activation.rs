//! Bringing the window back on Wayland with an activation token from the tray.
//!
//! Wayland gives a client no way to unminimise or raise its own window: winit's
//! `set_minimized(false)` and `focus_window` are both no-ops there. The one way is
//! `xdg_activation_v1`: a window is activated with a token the compositor handed someone the user
//! just interacted with. A click on the tray icon is such an interaction, and the tray host
//! (Plasma, GNOME's AppIndicator extension) passes its token on to the item just before the click
//! (`ksni::current_xdg_activation_token`, kept by `tray`). Activating with it unminimises the
//! window and gives it focus, with no permission beyond the Wayland socket — which is what lets
//! the Flatpak, which cannot reach KWin's scripting (`kwin`), bring its window back.
//!
//! Set up the way `blur`'s Wayland half is: winit's own `wl_display` and `wl_surface`, adopted as
//! a guest, and the global bound once at startup. Off Wayland, or on a compositor without the
//! protocol, it does nothing.

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry::WlRegistry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

struct State;

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

wayland_client::delegate_noop!(State: ignore WlSurface);
wayland_client::delegate_noop!(State: ignore XdgActivationV1);

pub struct Activation {
    inner: Option<Inner>,
}

struct Inner {
    conn: Connection,
    /// Kept, not read: the registry's events still have this queue to go to.
    _queue: EventQueue<State>,
    manager: XdgActivationV1,
    surface: WlSurface,
}

impl Activation {
    pub fn new(handles: &(impl HasWindowHandle + HasDisplayHandle)) -> Self {
        Self {
            inner: Inner::new(handles),
        }
    }

    /// Activate the window with `token`: unminimised, raised and focused, as the compositor
    /// allows. Nothing happens off Wayland or without the protocol.
    pub fn activate(&self, token: &str) {
        let Some(inner) = &self.inner else {
            return;
        };
        inner.manager.activate(token.to_owned(), &inner.surface);
        if let Err(e) = inner.conn.flush() {
            tracing::debug!(%e, "activation: could not send the request");
        }
    }
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
        // One roundtrip, at startup, as `blur` does; after this the module only writes.
        let (globals, queue) = registry_queue_init::<State>(&conn).ok()?;
        let manager: XdgActivationV1 = globals.bind(&queue.handle(), 1..=1, ()).ok()?;
        // SAFETY: winit's own `wl_surface`, alive as long as the window. `from_ptr` validates
        // the interface against the proxy's class, so a mismatch is an error.
        let id =
            unsafe { ObjectId::from_ptr(WlSurface::interface(), window.surface.as_ptr().cast()) }
                .ok()?;
        let surface = WlSurface::from_id(&conn, id).ok()?;
        Some(Self {
            conn,
            _queue: queue,
            manager,
            surface,
        })
    }
}
