//! Running inside a Flatpak sandbox.
//!
//! Flatpak sets `FLATPAK_ID` to the app's id (`io.github.AtotheY.moosik`) and nothing else here
//! needs to know more. The id matters because the desktop matches a window and a media player to
//! the launcher entry by it, and the entry in a Flatpak is named after the id, not "moosik".

/// The id the desktop knows this app by: the Flatpak id inside a sandbox, else `moosik`.
pub fn app_id() -> String {
    std::env::var("FLATPAK_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "moosik".to_owned())
}

/// Whether this process runs inside a Flatpak sandbox.
///
/// The sandbox cannot see a browser's profile folder, so anything that reads one finds nothing.
pub fn sandboxed() -> bool {
    std::env::var_os("FLATPAK_ID").is_some_and(|id| !id.is_empty())
}
