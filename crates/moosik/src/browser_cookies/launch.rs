//! Reopen the selected browser, which can differ from the default URL handler.
use super::Browser;
use anyhow::Context;
use anyhow::Result;
#[cfg(not(target_os = "linux"))]
use anyhow::ensure;

const MUSIC: &str = "https://music.youtube.com/";

impl Browser {
    #[cfg(target_os = "macos")]
    pub fn open_music(&self) -> Result<()> {
        let app = match self.label {
            "Chrome" => "Google Chrome",
            "Brave" => "Brave Browser",
            "Edge" => "Microsoft Edge",
            label => label,
        };
        let status = std::process::Command::new("/usr/bin/open")
            .args(["-a", app, MUSIC])
            .status()
            .context("could not start the macOS app launcher")?;
        ensure!(status.success(), "macOS could not find the browser");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn open_music(&self) -> Result<()> {
        use gtk::gio::{AppLaunchContext, DesktopAppInfo, prelude::*};

        // Match the selected install when native, Snap and Flatpak profiles coexist.
        let (_, root) = self
            .arg
            .split_once(':')
            .context("missing browser profile")?;
        let ids = if let Some((_, suffix)) = root.rsplit_once("/.var/app/") {
            vec![format!(
                "{}.desktop",
                suffix.split('/').next().unwrap_or(suffix)
            )]
        } else if let Some((_, suffix)) = root.rsplit_once("/snap/") {
            let snap = suffix.split('/').next().unwrap_or(suffix);
            vec![format!("{snap}_{snap}.desktop")]
        } else {
            let names: &[&str] = match self.label {
                "Firefox" => &["firefox", "org.mozilla.firefox", "firefox-esr"],
                "Chrome" => &["google-chrome"],
                "Chromium" => &["chromium", "chromium-browser"],
                "Brave" => &["brave-browser", "com.brave.Browser"],
                "Zen" => &["zen", "app.zen_browser.zen"],
                "LibreWolf" => &["librewolf", "io.gitlab.librewolf-community"],
                "Floorp" => &["floorp", "one.ablaze.floorp"],
                "Waterfox" => &["waterfox"],
                "Vivaldi" => &["vivaldi-stable", "vivaldi"],
                "Edge" => &["microsoft-edge"],
                "Opera" => &["opera"],
                "Whale" => &["naver-whale"],
                "Helium" => &["helium", "net.imput.helium"],
                _ => anyhow::bail!("unknown browser"),
            };
            names.iter().map(|name| format!("{name}.desktop")).collect()
        };
        let app = ids
            .iter()
            .find_map(|id| DesktopAppInfo::new(id))
            .context("could not find the browser in the desktop launcher")?;
        app.launch_uris(&[MUSIC], None::<&AppLaunchContext>)?;
        Ok(())
    }

    #[cfg(windows)]
    pub fn open_music(&self) -> Result<()> {
        // Chromium shares chrome.exe with Chrome. Its profile lives alongside
        // its own Application directory on a standard per-user installation.
        let exe = if self.label == "Chromium" {
            let (_, root) = self
                .arg
                .split_once(':')
                .context("missing Chromium profile")?;
            std::path::Path::new(root)
                .parent()
                .context("missing Chromium install")?
                .join("Application")
                .join("chrome.exe")
        } else {
            let name = if self.label == "Edge" {
                "msedge"
            } else {
                self.label
            };
            registered_executable(name)?
        };
        ensure!(
            exe.is_absolute() && exe.is_file(),
            "could not find the installed browser"
        );
        std::process::Command::new(exe).arg(MUSIC).spawn()?;
        Ok(())
    }
}

// A bare exe name can resolve in the working directory. Ask Windows for the
// registered path before launching a browser with a sign-in URL.
#[cfg(windows)]
fn registered_executable(name: &str) -> Result<std::path::PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows_sys::Win32::UI::Shell::{
        ASSOCF_OPEN_BYEXENAME, ASSOCSTR_EXECUTABLE, AssocQueryStringW,
    };
    let name: Vec<_> = format!("{name}.exe")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut path = vec![0u16; 32768];
    let mut len = path.len() as u32;
    let hr = unsafe {
        AssocQueryStringW(
            ASSOCF_OPEN_BYEXENAME,
            ASSOCSTR_EXECUTABLE,
            name.as_ptr(),
            std::ptr::null(),
            path.as_mut_ptr(),
            &mut len,
        )
    };
    ensure!(
        hr == 0 && len > 1,
        "Windows could not find the registered browser"
    );
    Ok(OsString::from_wide(&path[..len as usize - 1]).into())
}
