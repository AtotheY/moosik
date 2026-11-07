use std::path::Path;

pub const ARG: &str = "--spotify-login-window";

#[cfg(target_os = "windows")]
pub async fn capture() -> anyhow::Result<String> {
    windows::capture().await
}

#[cfg(not(target_os = "windows"))]
pub async fn capture() -> anyhow::Result<String> {
    anyhow::bail!("Spotify sign-in requires Windows WebView2")
}

#[cfg(target_os = "windows")]
pub fn capture_here(output: &Path) -> anyhow::Result<()> {
    windows::capture_here(output)
}

#[cfg(not(target_os = "windows"))]
pub fn capture_here(_output: &Path) -> anyhow::Result<()> {
    anyhow::bail!("Spotify sign-in requires Windows WebView2")
}

#[cfg(target_os = "windows")]
mod windows {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::FromRawHandle;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result, anyhow, bail, ensure};
    use tao::dpi::LogicalSize;
    use tao::event::{Event, StartCause, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::window::WindowBuilder;
    use wry::{NewWindowResponse, PageLoadEvent, WebContext, WebView, WebViewBuilder};

    const LOGIN: &str =
        "https://accounts.spotify.com/en/login?continue=https%3A%2F%2Fopen.spotify.com%2F";
    const COOKIE_URL: &str = "https://open.spotify.com/";
    const CANCELED: &str = "Spotify sign-in was canceled or did not finish";
    const MAX_COOKIE: u64 = 64 * 1024;

    struct Cleanup(PathBuf);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        descriptor: *mut c_void,
        inherit: i32,
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: u32,
            descriptor: *mut *mut c_void,
            length: *mut u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            path: *const u16,
            access: u32,
            share: u32,
            attributes: *mut SecurityAttributes,
            disposition: u32,
            flags: u32,
            template: *mut c_void,
        ) -> *mut c_void;
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(algorithm: *mut c_void, output: *mut u8, length: u32, flags: u32)
        -> i32;
    }

    pub async fn capture() -> Result<String> {
        let directory = config_dir()?;
        std::fs::create_dir_all(&directory).context("Could not create Spotify sign-in storage")?;
        let mut nonce = [0u8; 16];
        ensure!(
            unsafe {
                BCryptGenRandom(
                    std::ptr::null_mut(),
                    nonce.as_mut_ptr(),
                    nonce.len() as u32,
                    2,
                )
            } >= 0,
            "Could not create a private Spotify sign-in handoff"
        );
        let name: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = directory.join(format!("spotify-login-{name}.json"));
        drop(create_private(&path)?);
        let _cleanup = Cleanup(path.clone());
        let executable = std::env::current_exe().context("Could not find Moosik's executable")?;
        let mut child = tokio::process::Command::new(executable)
            .arg(super::ARG)
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .kill_on_drop(true)
            .spawn()
            .context("Could not open Spotify sign-in")?;
        let status = child.wait().await.context(CANCELED)?;
        ensure!(status.success(), CANCELED);
        let mut encoded = String::new();
        File::open(&path)
            .context("Spotify sign-in returned no session")?
            .take(MAX_COOKIE + 1)
            .read_to_string(&mut encoded)
            .context("Could not read the Spotify sign-in handoff")?;
        ensure!(
            encoded.len() as u64 <= MAX_COOKIE,
            "Spotify sign-in returned an invalid session"
        );
        let saved: serde_json::Value = serde_json::from_str(&encoded)
            .context("Spotify sign-in returned an invalid session")?;
        let cookie = saved["cookie"]
            .as_str()
            .context("Spotify sign-in returned no session")?;
        ensure!(
            valid_header(cookie),
            "Spotify sign-in returned an invalid session"
        );
        Ok(cookie.to_owned())
    }

    pub fn capture_here(output: &Path) -> Result<()> {
        validate_output(output)?;
        let cookie = run_window()?;
        let mut file = OpenOptions::new()
            .write(true)
            .share_mode(1 | 4)
            .open(output)
            .context("Could not open the private Spotify sign-in handoff")?;
        ensure!(
            file.metadata()?.len() == 0,
            "Spotify sign-in handoff is already in use"
        );
        file.write_all(&serde_json::to_vec(&serde_json::json!({"cookie": cookie}))?)
            .context("Could not write the Spotify sign-in handoff")?;
        file.sync_all()
            .context("Could not save the Spotify sign-in handoff")
    }

    fn config_dir() -> Result<PathBuf> {
        crate::profile_dirs()
            .map(|dirs| dirs.config_dir().to_owned())
            .context("Could not find Spotify sign-in storage")
    }

    fn validate_output(output: &Path) -> Result<()> {
        let name = output
            .file_name()
            .and_then(|name| name.to_str())
            .context("Spotify sign-in handoff is invalid")?;
        let nonce = name
            .strip_prefix("spotify-login-")
            .and_then(|name| name.strip_suffix(".json"))
            .context("Spotify sign-in handoff is invalid")?;
        ensure!(
            nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Spotify sign-in handoff is invalid"
        );
        let parent = output
            .parent()
            .context("Spotify sign-in handoff is invalid")?;
        ensure!(
            std::fs::canonicalize(parent)? == std::fs::canonicalize(config_dir()?)?,
            "Spotify sign-in handoff is outside its storage"
        );
        let metadata =
            std::fs::symlink_metadata(output).context("Spotify sign-in handoff is missing")?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() == 0,
            "Spotify sign-in handoff is invalid"
        );
        Ok(())
    }

    fn create_private(path: &Path) -> Result<File> {
        let descriptor_text: Vec<u16> = "D:P(A;;FA;;;OW)".encode_utf16().chain(Some(0)).collect();
        let mut descriptor = std::ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                descriptor_text.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error())
                .context("Could not protect the Spotify sign-in handoff");
        }
        let mut attributes = SecurityAttributes {
            length: std::mem::size_of::<SecurityAttributes>() as u32,
            descriptor,
            inherit: 0,
        };
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0x4000_0000,
                1 | 4,
                &mut attributes,
                1,
                0x80,
                std::ptr::null_mut(),
            )
        };
        let error = std::io::Error::last_os_error();
        unsafe {
            LocalFree(descriptor);
        }
        if handle as isize == -1 {
            return Err(error).context("Could not create the private Spotify sign-in handoff");
        }
        Ok(unsafe { File::from_raw_handle(handle) })
    }

    fn run_window() -> Result<String> {
        let profile = config_dir()?.join("spotify-login");
        std::fs::create_dir_all(&profile)
            .context("Could not create the Spotify sign-in profile")?;
        let mut event_loop = EventLoopBuilder::new().build();
        let window = WindowBuilder::new()
            .with_title("Sign in to Spotify")
            .with_inner_size(LogicalSize::new(560.0, 780.0))
            .with_min_inner_size(LogicalSize::new(420.0, 560.0))
            .with_visible(true)
            .build(&event_loop)
            .context("Could not create the Spotify sign-in window")?;
        let mut context = WebContext::new(Some(profile));
        let accounts_seen = Rc::new(Cell::new(false));
        let returned = Rc::new(Cell::new(false));
        let on_load = {
            let accounts_seen = Rc::clone(&accounts_seen);
            let returned = Rc::clone(&returned);
            move |event: PageLoadEvent, value: String| {
                let Ok(url) = url::Url::parse(&value) else {
                    return;
                };
                if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
                {
                    return;
                }
                match event {
                    PageLoadEvent::Started => {
                        returned.set(false);
                        if url.host_str() == Some("accounts.spotify.com") {
                            accounts_seen.set(true);
                        }
                    }
                    PageLoadEvent::Finished => {
                        returned
                            .set(accounts_seen.get() && url.host_str() == Some("open.spotify.com"));
                    }
                }
            }
        };
        let webview = WebViewBuilder::new_with_web_context(&mut context)
            .with_url(LOGIN)
            .with_on_page_load_handler(on_load)
            .with_navigation_handler(|value| value == "about:blank" || auth_url(&value))
            .with_new_window_req_handler(|value, _| if auth_url(&value) { NewWindowResponse::Allow } else { NewWindowResponse::Deny })
            .with_download_started_handler(|_, _| false)
            .with_devtools(false)
            .build(&window)
            .context("Could not create Spotify sign-in. Install the Microsoft Edge WebView2 runtime and try again")?;
        let mut outcome = None;
        let mut next = Instant::now() + Duration::from_millis(300);
        event_loop.run_return(|event, _, control_flow| {
            *control_flow = ControlFlow::WaitUntil(next);
            match event {
                Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                    if !returned.get() {
                        next = Instant::now() + Duration::from_millis(500);
                        return;
                    }
                    match session_cookie(&webview) {
                        Ok(Some(cookie)) => {
                            outcome = Some(Ok(cookie));
                            *control_flow = ControlFlow::Exit;
                        }
                        Ok(None) => {
                            next = Instant::now() + Duration::from_millis(500);
                        }
                        Err(_) => {
                            outcome =
                                Some(Err(anyhow!("Could not read the Spotify sign-in session")));
                            *control_flow = ControlFlow::Exit;
                        }
                    }
                }
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => {
                    outcome = Some(Err(anyhow!(CANCELED)));
                    *control_flow = ControlFlow::Exit;
                }
                _ => {}
            }
        });
        outcome.unwrap_or_else(|| bail!(CANCELED))
    }

    fn auth_url(value: &str) -> bool {
        url::Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none_or(|port| port == 443)
                && matches!(
                    url.host_str(),
                    Some(
                        "accounts.spotify.com"
                            | "open.spotify.com"
                            | "www.spotify.com"
                            | "spotify.com"
                            | "accounts.google.com"
                            | "appleid.apple.com"
                            | "account.apple.com"
                            | "idmsa.apple.com"
                    )
                )
        })
    }

    fn session_cookie(webview: &WebView) -> Result<Option<String>> {
        let cookies = webview.cookies_for_url(COOKIE_URL)?;
        let mut parts = Vec::new();
        for name in ["sp_dc", "sp_key"] {
            if let Some(cookie) = cookies
                .iter()
                .find(|cookie| cookie.name() == name && !cookie.value().is_empty())
            {
                ensure!(
                    cookie
                        .value()
                        .bytes()
                        .all(|byte| !byte.is_ascii_control() && byte != b';'),
                    "Spotify session cookie is invalid"
                );
                parts.push(format!("{name}={}", cookie.value()));
            } else if name == "sp_dc" {
                return Ok(None);
            }
        }
        Ok(Some(parts.join("; ")))
    }

    fn valid_header(cookie: &str) -> bool {
        let mut parts = cookie.split("; ");
        let valid = |part: &str, expected: &str| {
            part.split_once('=').is_some_and(|(name, value)| {
                name == expected
                    && !value.is_empty()
                    && value.bytes().all(|byte| {
                        byte.is_ascii_graphic() && !matches!(byte, b'"' | b',' | b';' | b'\\')
                    })
            })
        };
        parts.next().is_some_and(|part| valid(part, "sp_dc"))
            && parts.next().is_none_or(|part| valid(part, "sp_key"))
            && parts.next().is_none()
    }

    #[cfg(test)]
    mod tests {
        use super::{auth_url, valid_header};

        #[test]
        fn captured_sessions_only_contain_the_expected_cookies() {
            assert!(valid_header("sp_dc=session"));
            assert!(valid_header("sp_dc=session; sp_key=device"));
            for value in [
                "sp_dc=",
                "sp_dc=session;sp_key=device",
                "sp_dc=session; sp_dc=other",
                "sp_dc=session; sp_key=",
                "sp_dc=session\r\nheader:value",
                "sp_dc=session; sp_key=device; extra=value",
                "sp_dc=\"session\"",
                "sp_dc=session value",
            ] {
                assert!(!valid_header(value));
            }
        }

        #[test]
        fn sign_in_navigation_requires_a_known_secure_origin() {
            assert!(auth_url("https://accounts.spotify.com/en/login"));
            assert!(auth_url("https://accounts.google.com/"));
            for value in [
                "http://accounts.spotify.com/",
                "https://accounts.spotify.com.evil.test/",
                "https://accounts.spotify.com:444/",
                "https://name@accounts.spotify.com/",
            ] {
                assert!(!auth_url(value));
            }
        }
    }
}
