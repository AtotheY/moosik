pub const ARG: &str = "--spotify-drm-window";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RendererCommand {
    Load { prefix: String },
    Stop,
}

#[cfg(any(target_os = "windows", test))]
fn playback_path(value: &str) -> bool {
    value
        .strip_prefix('/')
        .and_then(|value| value.strip_suffix('/'))
        .is_some_and(|capability| {
            capability.len() == 64
                && capability
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
}

#[cfg(any(target_os = "windows", test))]
fn playback_page(value: &str) -> anyhow::Result<url::Url> {
    use anyhow::{Context, ensure};

    let url = url::Url::parse(value).context("Spotify audio address is invalid")?;
    ensure!(
        url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port().is_some_and(|port| port != 0)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && playback_path(url.path()),
        "Spotify audio address must use the local playback page"
    );
    Ok(url)
}

#[cfg(any(target_os = "windows", test))]
fn navigation_allowed(page: &str, value: &str) -> bool {
    value == "about:blank" || value == page
}

#[cfg(any(target_os = "windows", test))]
impl RendererCommand {
    fn script(self) -> Option<String> {
        match self {
            Self::Load { prefix } if playback_path(&prefix) => {
                Some(format!("window.moosikLoad('{prefix}')"))
            }
            Self::Load { .. } => None,
            Self::Stop => Some("window.moosikStop()".into()),
        }
    }
}

#[cfg(any(target_os = "windows", test))]
fn read_commands(
    mut input: impl std::io::BufRead,
    mut receive: impl FnMut(RendererCommand) -> bool,
) -> std::io::Result<()> {
    use std::io::{BufRead, Read};

    const MAX_LINE: u64 = 8192;
    loop {
        let mut line = Vec::new();
        let read = (&mut input)
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(());
        }
        if read as u64 > MAX_LINE {
            if line.last() != Some(&b'\n') {
                input.skip_until(b'\n')?;
            }
            continue;
        }
        if let Ok(command) = serde_json::from_slice(&line)
            && !receive(command)
        {
            return Ok(());
        }
    }
}

#[cfg(target_os = "windows")]
pub fn run(url: &str) -> anyhow::Result<()> {
    windows::run(url)
}

#[cfg(not(target_os = "windows"))]
pub fn run(_url: &str) -> anyhow::Result<()> {
    anyhow::bail!("Protected Spotify playback requires Windows WebView2")
}

#[cfg(target_os = "windows")]
mod windows {
    use std::io::BufReader;
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result};
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::platform::windows::WindowBuilderExtWindows;
    use tao::window::WindowBuilder;
    use wry::{
        NewWindowResponse, PageLoadEvent, WebContext, WebViewBuilder, WebViewBuilderExtWindows,
    };

    use super::{navigation_allowed, playback_page, read_commands};

    enum RendererEvent {
        Script(String),
        Ready,
        Exit,
    }

    pub fn run(value: &str) -> Result<()> {
        let page = playback_page(value)?.to_string();
        let profile = crate::profile_dirs()
            .context("Could not find Spotify audio storage")?
            .config_dir()
            .join("spotify-drm");
        std::fs::create_dir_all(&profile).context("Could not create Spotify audio storage")?;
        let mut event_loop = EventLoopBuilder::<RendererEvent>::with_user_event().build();
        let proxy = event_loop.create_proxy();
        let ready_proxy = proxy.clone();
        let script_proxy = proxy.clone();
        std::thread::Builder::new()
            .name("spotify-audio-parent".into())
            .spawn(move || {
                let input = std::io::stdin().lock();
                let _ = read_commands(BufReader::new(input), |command| {
                    command.script().is_none_or(|script| {
                        proxy.send_event(RendererEvent::Script(script)).is_ok()
                    })
                });
                let _ = proxy.send_event(RendererEvent::Exit);
            })
            .context("Could not watch the Spotify audio parent")?;
        let window = WindowBuilder::new()
            .with_title("Moosik audio")
            .with_visible(false)
            .with_skip_taskbar(true)
            .build(&event_loop)
            .context("Could not create the Spotify audio window")?;
        let mut context = WebContext::new(Some(profile));
        let webview = WebViewBuilder::new_with_web_context(&mut context)
            .with_additional_browser_args(
                "--disable-gpu --autoplay-policy=no-user-gesture-required --disable-background-timer-throttling \
                 --disable-features=HardwareMediaKeyHandling",
            )
            .with_url(&page)
            .with_navigation_handler({
                let page = page.clone();
                move |value| navigation_allowed(&page, &value)
            })
            .with_on_page_load_handler(move |event, url| {
                if matches!(event, PageLoadEvent::Finished) && url == page {
                    let _ = ready_proxy.send_event(RendererEvent::Ready);
                }
            })
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_devtools(false)
            .build(&window)
            .context("Could not create the Spotify audio renderer")?;
        let mut ready = false;
        let mut pending = None;
        let ready_deadline = Instant::now() + Duration::from_secs(30);
        event_loop.run_return(|event, _, control_flow| {
            match event {
                Event::UserEvent(RendererEvent::Script(script)) => pending = Some(script),
                Event::UserEvent(RendererEvent::Ready) => ready = true,
                Event::UserEvent(RendererEvent::Exit)
                | Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                _ => {}
            }
            if !ready && Instant::now() >= ready_deadline {
                *control_flow = ControlFlow::Exit;
                return;
            }
            *control_flow = if ready {
                ControlFlow::Wait
            } else {
                ControlFlow::WaitUntil(ready_deadline)
            };
            if ready && let Some(script) = pending.take() {
                let proxy = script_proxy.clone();
                let result = webview.evaluate_script_with_callback(
                    &format!("({script}, true)"),
                    move |result| {
                        if result != "true" {
                            let _ = proxy.send_event(RendererEvent::Exit);
                        }
                    },
                );
                if result.is_err() {
                    *control_flow = ControlFlow::Exit;
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef/";
    const PAGE: &str =
        "http://127.0.0.1:32123/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef/";

    fn commands(input: &[u8]) -> Vec<RendererCommand> {
        let mut commands = Vec::new();
        read_commands(std::io::Cursor::new(input), |command| {
            commands.push(command);
            true
        })
        .unwrap();
        commands
    }

    #[test]
    fn renderer_commands_round_trip_as_json_lines() {
        let mut input = serde_json::to_vec(&RendererCommand::Load {
            prefix: PREFIX.to_owned(),
        })
        .unwrap();
        input.push(b'\n');
        input.extend(serde_json::to_vec(&RendererCommand::Stop).unwrap());
        input.push(b'\n');
        assert!(matches!(
            commands(&input).as_slice(),
            [RendererCommand::Load { prefix }, RendererCommand::Stop] if prefix == PREFIX
        ));
        assert_eq!(
            serde_json::to_value(RendererCommand::Stop).unwrap(),
            serde_json::json!({"kind": "stop"})
        );
    }

    #[test]
    fn malformed_commands_are_skipped_without_losing_the_next_line() {
        let input = b"not json\n{\"kind\":\"other\"}\n{\"kind\":\"load\"}\n{\"kind\":\"load\",\"prefix\":false}\n{\"kind\":\"stop\"}\n";
        assert!(matches!(
            commands(input).as_slice(),
            [RendererCommand::Stop]
        ));
        assert!(commands(b"{\"kind\":").is_empty());
        assert!(commands(b"").is_empty());
    }

    #[test]
    fn oversized_commands_are_discarded_with_bounded_lines() {
        let mut input = b"{\"kind\":\"stop\"}".to_vec();
        input.resize(8191, b' ');
        input.push(b'\n');
        input.extend_from_slice(b"{\"kind\":\"stop\"}");
        input.resize(input.len() + 8192, b' ');
        input.push(b'\n');
        input.extend_from_slice(b"{\"kind\":\"stop\"}");
        input.resize(input.len() + 20000, b' ');
        input.push(b'\n');
        input.extend_from_slice(b"{\"kind\":\"stop\"}\n");
        assert!(matches!(
            commands(&input).as_slice(),
            [RendererCommand::Stop, RendererCommand::Stop]
        ));
    }

    #[test]
    fn renderer_reader_stops_when_the_event_loop_closes() {
        let mut received = 0;
        read_commands(
            std::io::Cursor::new(b"{\"kind\":\"stop\"}\n{\"kind\":\"stop\"}\n"),
            |_| {
                received += 1;
                false
            },
        )
        .unwrap();
        assert_eq!(received, 1);
    }

    #[test]
    fn page_requires_a_loopback_port_and_capability() {
        assert_eq!(playback_page(PAGE).unwrap().as_str(), PAGE);
        for value in [
            format!("https://127.0.0.1:32123{PREFIX}"),
            format!("http://127.0.0.1{PREFIX}"),
            format!("http://127.0.0.1:0{PREFIX}"),
            format!("http://localhost:32123{PREFIX}"),
            format!("http://192.168.1.1:32123{PREFIX}"),
            format!("http://user:password@127.0.0.1:32123{PREFIX}"),
            format!("{PAGE}?query=1"),
            format!("{PAGE}#fragment"),
            format!("{PAGE}media"),
            PAGE.trim_end_matches('/').to_owned(),
            "http://127.0.0.1:32123/short/".into(),
        ] {
            assert!(playback_page(&value).is_err(), "{value}");
        }
    }

    #[test]
    fn navigation_allows_only_the_page_and_initial_blank() {
        assert!(navigation_allowed(PAGE, PAGE));
        assert!(navigation_allowed(PAGE, "about:blank"));
        assert!(!navigation_allowed(PAGE, &format!("{PAGE}media")));
        assert!(!navigation_allowed(PAGE, "about:blank#fragment"));
    }

    #[test]
    fn load_scripts_only_accept_track_capabilities() {
        let script = |prefix: &str| {
            RendererCommand::Load {
                prefix: prefix.into(),
            }
            .script()
        };
        assert_eq!(
            script(PREFIX),
            Some(format!("window.moosikLoad('{PREFIX}')"))
        );
        for value in [PAGE, "/", "/../", "'); alert('x", "/abc/", "/ABC/"] {
            assert!(script(value).is_none(), "{value}");
        }
    }
}
