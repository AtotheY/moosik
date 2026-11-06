//! Signed-bundle update verification with dummy data; never reads the user's account.
fn main() -> eframe::Result {
    #[cfg(target_os = "macos")]
    {
        moosik::update::diagnostic::run()
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("The native updater probe runs on macOS.");
        Ok(())
    }
}
