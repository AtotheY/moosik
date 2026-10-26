//! "Start on boot".
//!
//! Very different mechanisms for one setting: an XDG autostart entry on Linux (matching the
//! Linux-first install path in `scripts/appimage-install.sh`), a per-user LaunchAgent on macOS,
//! and the per-user `Run` key on Windows — the registry location Task Manager's own Startup tab reads, so the switch can be
//! audited and turned off from outside the app the same way every other Windows app's is.
//!
//! Both entries pass [`ARG`], so a launch can tell it came from login and ask the compositor to
//! bring its window forward once it is up. That matters on Wayland, where nothing can raise a
//! window that has not asked: see `App::request_foreground`.

/// The argument the autostart entry passes, telling the app this launch came from login.
pub const ARG: &str = "--autostart";

#[cfg(target_os = "linux")]
mod linux {
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result};

    use super::ARG;

    fn path() -> Result<PathBuf> {
        let dirs = directories::BaseDirs::new().context("could not find the config directory")?;
        Ok(dirs.config_dir().join("autostart").join("refryt.desktop"))
    }

    /// The binary to launch on login.
    ///
    /// Not simply `Exec=refryt`, the way the application-launcher entry gets away with it: that
    /// one assumes whatever installed it also put `refryt` on `PATH`, which is true for the
    /// AppImage installer's own symlink-free copy into `~/.local/bin` today but is not a safe
    /// assumption in general — and `std::env::current_exe()` alone is worse for the AppImage case
    /// specifically, since an AppImage runs from a temporary squashfs mount that stops existing
    /// the moment this process exits, so writing *that* path would autostart something that is
    /// gone by next login. AppImage's own runtime sets `$APPIMAGE` to the real, persistent
    /// `.AppImage` file it was launched from, so that takes priority; `current_exe()` is the
    /// fallback for every other way this binary might be installed.
    fn exec_path() -> Option<String> {
        std::env::var("APPIMAGE").ok().or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string())
        })
    }

    /// One `Exec` argument, quoted the way the desktop entry spec asks: the whole value in double
    /// quotes with `\`, `"`, `` ` `` and `$` escaped, and a literal `%` doubled — a bare `%` is
    /// the spec's field-code prefix and would be expanded by the launcher (to nothing, for any
    /// code but `%%`).
    fn quoted(arg: &str) -> String {
        let mut out = String::with_capacity(arg.len() + 2);
        out.push('"');
        for c in arg.chars() {
            match c {
                '\\' | '"' | '`' | '$' => {
                    out.push('\\');
                    out.push(c);
                }
                '%' => out.push_str("%%"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// The exact `Exec` line this version writes, used both to build the entry and to recognise
    /// an entry that already matches.
    fn exec_line(exec: &str) -> String {
        format!("Exec={} {ARG}", quoted(exec))
    }

    fn desktop_entry(exec: &str) -> String {
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Moosik\n\
             Comment=A fast, native YouTube Music client\n\
             {}\n\
             Icon=refryt\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n",
            exec_line(exec)
        )
    }

    /// The path an entry's `Exec=` line runs, unquoted. Enough of the spec for our own entries
    /// (one quoted argument, then [`ARG`]) and for the unquoted `Exec=refryt` a hand-written
    /// entry would carry; anything with more elaborate quoting parses to `None` and is left
    /// alone.
    fn entry_exec(text: &str) -> Option<String> {
        let value = text.lines().find_map(|l| l.strip_prefix("Exec="))?.trim();
        let arg = if let Some(rest) = value.strip_prefix('"') {
            rest.split('"').next()?
        } else {
            value.split_whitespace().next()?
        };
        Some(arg.replace("%%", "%"))
    }

    fn write_entry(path: &Path, exec: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        std::fs::write(path, desktop_entry(exec))
            .with_context(|| format!("could not write {}", path.display()))
    }

    /// Quiet if it was never there — turning the setting on and off without ever having
    /// restarted in between should not surface an error either way.
    fn remove_entry(path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("could not remove {}", path.display())),
        }
    }

    /// Write the autostart entry. Called when "Start on boot" is turned on.
    pub fn enable() -> Result<()> {
        let Some(exec) = exec_path() else {
            anyhow::bail!("could not determine this binary's own path");
        };
        write_entry(&path()?, &exec)
    }

    /// Remove the autostart entry. Called when "Start on boot" is turned off.
    pub fn disable() -> Result<()> {
        remove_entry(&path()?)
    }

    /// Keep the entry in step with the setting, checked once at startup as well as on a toggle.
    /// Startup is what heals an entry an earlier version wrote — a path that no longer exists, or
    /// one without [`ARG`] — and what removes an entry the setting no longer wants.
    ///
    /// The entry is only rewritten when it points at *this* binary, or at nothing that still
    /// exists: running a development build against the real profile must not hijack the
    /// installed app's own autostart entry.
    pub fn sync(enabled: bool) -> Result<()> {
        let Some(exec) = exec_path() else {
            // Nowhere to point an entry; leave whatever is there alone.
            return Ok(());
        };
        sync_at(&path()?, &exec, enabled)
    }

    /// [`sync`], with the entry path and binary passed in — pure filesystem work, so the
    /// branches are testable without touching the real profile.
    fn sync_at(path: &Path, exec: &str, enabled: bool) -> Result<()> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return if enabled {
                    write_entry(path, exec)
                } else {
                    Ok(())
                };
            }
            Err(e) => {
                return Err(e).with_context(|| format!("could not read {}", path.display()));
            }
        };
        let current = entry_exec(&text);
        let points_here = current.as_deref() == Some(exec);
        if !enabled {
            // Only remove an entry this binary owns; someone else's is not ours to delete.
            return if points_here {
                remove_entry(path)
            } else {
                Ok(())
            };
        }
        if points_here && text.contains(&exec_line(exec)) {
            return Ok(());
        }
        if !points_here && current.is_some_and(|p| Path::new(&p).exists()) {
            // A different, still-working install's entry: leave it.
            return Ok(());
        }
        write_entry(path, exec)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_entry_names_the_binary_and_asks_for_the_window() {
            let entry = desktop_entry("/home/user/.local/bin/refryt.AppImage");
            assert!(entry.contains("Exec=\"/home/user/.local/bin/refryt.AppImage\" --autostart"));
            assert!(entry.contains("Type=Application"));
            assert!(entry.contains("X-GNOME-Autostart-enabled=true"));
        }

        #[test]
        fn exec_quoting_survives_spaces_and_field_codes() {
            assert_eq!(
                quoted("/home/user/My Apps/refryt"),
                "\"/home/user/My Apps/refryt\""
            );
            assert_eq!(quoted("/tmp/100%/refryt"), "\"/tmp/100%%/refryt\"");
            assert_eq!(quoted("/tmp/a\"b/refryt"), "\"/tmp/a\\\"b/refryt\"");
            assert_eq!(quoted("/tmp/a$b/refryt"), "\"/tmp/a\\$b/refryt\"");
        }

        /// The startup heal: a missing entry is written, an entry without the flag an older
        /// version wrote is replaced, a working entry for another install is left alone, and
        /// turning the setting off removes only an entry this binary owns.
        #[test]
        fn sync_heals_the_entry_without_hijacking_another_install() {
            let dir =
                std::env::temp_dir().join(format!("refryt-autostart-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("refryt.desktop");
            let ours = "/home/user/.local/bin/refryt.AppImage";
            let other = dir.join("other-refryt");
            std::fs::write(&other, "#!/bin/sh\n").unwrap();
            let other = other.display().to_string();

            sync_at(&path, ours, true).unwrap();
            assert!(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .contains(&exec_line(ours))
            );

            // Written by an older version, without the flag: replaced.
            std::fs::write(&path, desktop_entry(ours).replace(&format!(" {ARG}"), "")).unwrap();
            sync_at(&path, ours, true).unwrap();
            assert!(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .contains(&exec_line(ours))
            );

            // Another, still-existing install's entry: left alone, on and off.
            std::fs::write(&path, desktop_entry(&other)).unwrap();
            sync_at(&path, ours, true).unwrap();
            sync_at(&path, ours, false).unwrap();
            assert!(std::fs::read_to_string(&path).unwrap().contains(&other));

            // Ours: removed when off.
            std::fs::write(&path, desktop_entry(ours)).unwrap();
            sync_at(&path, ours, false).unwrap();
            assert!(!path.exists());

            let _ = std::fs::remove_dir_all(&dir);
        }

        /// The parser reads back exactly what the writer wrote, quoted path and all — and the
        /// unquoted `Exec=refryt` an entry written by hand would carry.
        #[test]
        fn the_exec_path_round_trips() {
            let entry = desktop_entry("/home/user/My Apps/refryt.AppImage");
            assert_eq!(
                entry_exec(&entry).as_deref(),
                Some("/home/user/My Apps/refryt.AppImage")
            );
            assert_eq!(entry_exec("Exec=refryt\n").as_deref(), Some("refryt"));
            assert_eq!(
                entry_exec("Exec=\"/tmp/100%%/refryt\" --autostart"),
                Some("/tmp/100%/refryt".to_owned())
            );
            assert_eq!(entry_exec("Name=Moosik\n"), None);
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::ptr;

    use anyhow::{Context, Result, bail};
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegCreateKeyExW,
        RegDeleteValueW, RegSetValueExW,
    };

    use super::ARG;

    /// The per-user Run key: this user only, which is the scope the setting is about, and no
    /// elevation needed to write it.
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    /// One fixed value name, so enabling twice overwrites rather than accumulates entries.
    const VALUE_NAME: &str = "refryt";

    /// What the Run value should say for a binary at `exe`.
    ///
    /// Quoted: a Run value is a command line, and an unquoted `C:\Program Files\...` path would
    /// be split at the first space. [`ARG`] rides along so the launch knows it came from login.
    /// Pure, so the quoting is testable without a registry.
    fn run_value(exe: &Path) -> String {
        format!("\"{}\" {ARG}", exe.display())
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(Some(0)).collect()
    }

    /// Open (creating if needed) the Run key for writing.
    fn open_key() -> Result<HKEY> {
        let subkey = wide(RUN_KEY);
        let mut key: HKEY = 0;
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                ptr::null(),
                0,
                KEY_SET_VALUE,
                ptr::null(),
                &mut key,
                ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            bail!("could not open HKCU\\{RUN_KEY} (error {status})");
        }
        Ok(key)
    }

    /// Point the Run key at this binary. Called when "Start on boot" is turned on.
    pub fn enable() -> Result<()> {
        let exe = std::env::current_exe().context("could not determine this binary's own path")?;
        let name = wide(VALUE_NAME);
        let data = wide(&run_value(&exe));
        let key = open_key()?;
        let status = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * size_of::<u16>()) as u32,
            )
        };
        unsafe { RegCloseKey(key) };
        if status != ERROR_SUCCESS {
            bail!("could not write the Run value (error {status})");
        }
        Ok(())
    }

    /// Remove the Run value. Quiet if it was never there, like the Linux backend.
    pub fn disable() -> Result<()> {
        let name = wide(VALUE_NAME);
        let key = open_key()?;
        let status = unsafe { RegDeleteValueW(key, name.as_ptr()) };
        unsafe { RegCloseKey(key) };
        if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
            bail!("could not remove the Run value (error {status})");
        }
        Ok(())
    }

    /// Keep the Run value in step with the setting, checked once at startup as well as on a
    /// toggle. Unlike the Linux entry there is no cheap read-back of the value, so "on" simply
    /// rewrites it (one fixed value name, so that is idempotent) and "off" removes it.
    pub fn sync(enabled: bool) -> Result<()> {
        if enabled { enable() } else { disable() }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_run_value_quotes_a_path_with_spaces_and_carries_the_flag() {
            let value = run_value(Path::new(r"C:\Program Files\refryt\refryt.exe"));
            assert_eq!(
                value,
                "\"C:\\Program Files\\refryt\\refryt.exe\" --autostart"
            );
        }
    }
}

/// macOS: a per-user LaunchAgent, which `launchd` loads at the next login. The same
/// ownership rules as the Linux entry apply — only an entry pointing at this binary (or at
/// nothing that still exists) is rewritten or removed.
#[cfg(target_os = "macos")]
mod macos {
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result};

    use super::ARG;

    const LABEL: &str = "io.github.gridoe.refryt";

    fn path() -> Result<PathBuf> {
        let dirs = directories::BaseDirs::new().context("could not find the home directory")?;
        Ok(dirs
            .home_dir()
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    /// The binary to launch on login: this executable, which inside an app bundle is
    /// `Moosik.app/Contents/MacOS/refryt`.
    fn exec_path() -> Option<String> {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(str::to_owned))
    }

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    fn unescape(s: &str) -> String {
        s.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    }

    fn plist(exec: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>Label</key>\n\
\t<string>{LABEL}</string>\n\
\t<key>ProgramArguments</key>\n\
\t<array>\n\
\t\t<string>{}</string>\n\
\t\t<string>{ARG}</string>\n\
\t</array>\n\
\t<key>RunAtLoad</key>\n\
\t<true/>\n\
</dict>\n\
</plist>\n",
            escape(exec)
        )
    }

    /// The executable a plist launches: the first `<string>` after `ProgramArguments`.
    fn plist_exec(text: &str) -> Option<String> {
        let rest = &text[text.find("<key>ProgramArguments</key>")?..];
        let start = rest.find("<string>")? + "<string>".len();
        let end = start + rest[start..].find("</string>")?;
        Some(unescape(&rest[start..end]))
    }

    fn write_entry(path: &Path, exec: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        std::fs::write(path, plist(exec))
            .with_context(|| format!("could not write {}", path.display()))
    }

    fn remove_entry(path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("could not remove {}", path.display())),
        }
    }

    /// Write the LaunchAgent. Called when "Start on boot" is turned on.
    pub fn enable() -> Result<()> {
        let Some(exec) = exec_path() else {
            anyhow::bail!("could not determine this binary's own path");
        };
        write_entry(&path()?, &exec)
    }

    /// Remove the LaunchAgent. Called when "Start on boot" is turned off.
    pub fn disable() -> Result<()> {
        remove_entry(&path()?)
    }

    /// Keep the entry in step with the setting; see the Linux module's `sync`.
    pub fn sync(enabled: bool) -> Result<()> {
        let Some(exec) = exec_path() else {
            return Ok(());
        };
        sync_at(&path()?, &exec, enabled)
    }

    fn sync_at(path: &Path, exec: &str, enabled: bool) -> Result<()> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return if enabled {
                    write_entry(path, exec)
                } else {
                    Ok(())
                };
            }
            Err(e) => {
                return Err(e).with_context(|| format!("could not read {}", path.display()));
            }
        };
        let current = plist_exec(&text);
        let points_here = current.as_deref() == Some(exec);
        if !enabled {
            return if points_here {
                remove_entry(path)
            } else {
                Ok(())
            };
        }
        if points_here && text.contains(ARG) {
            return Ok(());
        }
        if !points_here && current.is_some_and(|p| Path::new(&p).exists()) {
            return Ok(());
        }
        write_entry(path, exec)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn plist_round_trips_the_executable() {
            let exec = "/Applications/Re & Fry <t>.app/Contents/MacOS/refryt";
            let text = plist(exec);
            assert!(text.contains(ARG));
            assert!(text.contains("<key>RunAtLoad</key>"));
            assert_eq!(plist_exec(&text).as_deref(), Some(exec));
        }

        #[test]
        fn sync_creates_heals_and_removes() {
            let dir = std::env::temp_dir()
                .join(format!("refryt-autostart-test-mac-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let file = dir.join("a.plist");

            sync_at(&file, "/x/refryt", false).unwrap();
            assert!(!file.exists());

            sync_at(&file, "/x/refryt", true).unwrap();
            assert_eq!(
                plist_exec(&std::fs::read_to_string(&file).unwrap()).as_deref(),
                Some("/x/refryt")
            );

            // Another still-existing install's entry is left alone.
            let other = std::env::current_exe().unwrap();
            let other = other.to_str().unwrap();
            write_entry(&file, other).unwrap();
            sync_at(&file, "/x/refryt", true).unwrap();
            assert_eq!(
                plist_exec(&std::fs::read_to_string(&file).unwrap()).as_deref(),
                Some(other)
            );
            sync_at(&file, "/x/refryt", false).unwrap();
            assert!(file.exists());

            // A stale entry pointing at nothing is healed, then removed by its owner.
            write_entry(&file, "/gone/refryt").unwrap();
            sync_at(&file, "/x/refryt", true).unwrap();
            assert_eq!(
                plist_exec(&std::fs::read_to_string(&file).unwrap()).as_deref(),
                Some("/x/refryt")
            );
            sync_at(&file, "/x/refryt", false).unwrap();
            assert!(!file.exists());

            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{disable, enable, sync};

#[cfg(target_os = "macos")]
pub use macos::{disable, enable, sync};

#[cfg(target_os = "windows")]
pub use windows::{disable, enable, sync};
