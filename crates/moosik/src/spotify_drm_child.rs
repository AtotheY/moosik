pub const ARG: &str = "--spotify-drm-window";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RendererCommand {
    Load { url: String },
    Stop,
}

#[cfg(any(target_os = "windows", test))]
struct PlaybackTarget {
    url: url::Url,
    origin: url::Origin,
    prefix: String,
}

#[cfg(any(target_os = "windows", test))]
impl PlaybackTarget {
    fn parse(value: &str) -> anyhow::Result<Self> {
        use anyhow::{Context, ensure};

        let url = url::Url::parse(value).context("Spotify audio address is invalid")?;
        ensure!(
            url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.port().is_some_and(|port| port != 0)
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Spotify audio address must use the local playback server"
        );
        let capability = url
            .path_segments()
            .and_then(|mut segments| segments.next())
            .filter(|segment| {
                segment.len() >= 16
                    && segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            })
            .context("Spotify audio address has no playback capability")?;
        let prefix = format!("/{capability}/");
        ensure!(
            url.path().starts_with(&prefix),
            "Spotify audio address is outside the playback capability"
        );
        Ok(Self {
            origin: url.origin(),
            url,
            prefix,
        })
    }

    fn permits(&self, value: &str) -> bool {
        url::Url::parse(value).is_ok_and(|target| {
            target.origin() == self.origin
                && target.username().is_empty()
                && target.password().is_none()
                && target.path().starts_with(&self.prefix)
        })
    }
}

#[cfg(any(target_os = "windows", test))]
fn navigation_allowed(target: Option<&PlaybackTarget>, value: &str) -> bool {
    value == "about:blank" || target.is_some_and(|target| target.permits(value))
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
    use std::cell::RefCell;
    use std::io::BufReader;
    use std::rc::Rc;

    use anyhow::{Context, Result};
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::platform::windows::WindowBuilderExtWindows;
    use tao::window::WindowBuilder;
    use wry::{NewWindowResponse, WebContext, WebViewBuilder, WebViewBuilderExtWindows};

    use super::{PlaybackTarget, RendererCommand, navigation_allowed, read_commands};

    enum RendererEvent {
        Load(PlaybackTarget),
        Stop,
        Exit,
    }

    pub fn run(value: &str) -> Result<()> {
        let initial = PlaybackTarget::parse(value)?;
        let initial_url = initial.url.as_str().to_owned();
        let allowed = Rc::new(RefCell::new(Some(initial)));
        let profile = crate::profile_dirs()
            .context("Could not find Spotify audio storage")?
            .config_dir()
            .join("spotify-drm");
        std::fs::create_dir_all(&profile).context("Could not create Spotify audio storage")?;
        let mut event_loop = EventLoopBuilder::<RendererEvent>::with_user_event().build();
        let proxy = event_loop.create_proxy();
        std::thread::Builder::new()
            .name("spotify-audio-parent".into())
            .spawn(move || {
                let input = std::io::stdin().lock();
                let _ = read_commands(BufReader::new(input), |command| {
                    let event = match command {
                        RendererCommand::Load { url } => {
                            let Ok(target) = PlaybackTarget::parse(&url) else {
                                return true;
                            };
                            RendererEvent::Load(target)
                        }
                        RendererCommand::Stop => RendererEvent::Stop,
                    };
                    proxy.send_event(event).is_ok()
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
                "--autoplay-policy=no-user-gesture-required --disable-background-timer-throttling",
            )
            .with_url(&initial_url)
            .with_navigation_handler({
                let allowed = Rc::clone(&allowed);
                move |value| navigation_allowed(allowed.borrow().as_ref(), &value)
            })
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_devtools(false)
            .build(&window)
            .context("Could not create the Spotify audio renderer")?;
        event_loop.run_return(|event, _, control_flow| {
            *control_flow = ControlFlow::Wait;
            match event {
                Event::UserEvent(RendererEvent::Load(target)) => {
                    let url = target.url.as_str().to_owned();
                    *allowed.borrow_mut() = Some(target);
                    if webview.load_url(&url).is_err() {
                        *control_flow = ControlFlow::Exit;
                    }
                }
                Event::UserEvent(RendererEvent::Stop) => {
                    *allowed.borrow_mut() = None;
                    if webview.load_url("about:blank").is_err() {
                        *control_flow = ControlFlow::Exit;
                    }
                }
                Event::UserEvent(RendererEvent::Exit)
                | Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => *control_flow = ControlFlow::Exit,
                _ => {}
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str = "http://127.0.0.1:32123/abcdefghijklmnop/";
    const SECOND: &str = "http://127.0.0.1:32124/qrstuvwxyzABCDEF/";

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
            url: FIRST.to_owned(),
        })
        .unwrap();
        input.push(b'\n');
        input.extend(serde_json::to_vec(&RendererCommand::Stop).unwrap());
        input.push(b'\n');
        assert!(matches!(
            commands(&input).as_slice(),
            [RendererCommand::Load { url }, RendererCommand::Stop] if url == FIRST
        ));
        assert_eq!(
            serde_json::to_value(RendererCommand::Stop).unwrap(),
            serde_json::json!({"kind": "stop"})
        );
    }

    #[test]
    fn malformed_commands_are_skipped_without_losing_the_next_line() {
        let input = b"not json\n{\"kind\":\"other\"}\n{\"kind\":\"load\"}\n{\"kind\":\"load\",\"url\":false}\n{\"kind\":\"stop\"}\n";
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
    fn load_targets_require_a_loopback_port_and_capability() {
        assert_eq!(PlaybackTarget::parse(FIRST).unwrap().url.as_str(), FIRST);
        for value in [
            "https://127.0.0.1:32123/abcdefghijklmnop/",
            "http://127.0.0.1/abcdefghijklmnop/",
            "http://127.0.0.1:0/abcdefghijklmnop/",
            "http://localhost:32123/abcdefghijklmnop/",
            "http://192.168.1.1:32123/abcdefghijklmnop/",
            "http://user@127.0.0.1:32123/abcdefghijklmnop/",
            "http://user:password@127.0.0.1:32123/abcdefghijklmnop/",
            "http://127.0.0.1:32123/abcdefghijklmnop/?query=1",
            "http://127.0.0.1:32123/abcdefghijklmnop/#fragment",
            "http://127.0.0.1:32123/short/",
            "http://127.0.0.1:32123/abcdefghijklmno%21/",
            "http://127.0.0.1:32123/abcdefghijklmnop",
            "http://127.0.0.1:32123/",
            "about:blank",
            "invalid",
        ] {
            assert!(PlaybackTarget::parse(value).is_err(), "{value}");
        }
    }

    #[test]
    fn navigation_follows_only_the_current_target_and_blank_stop() {
        let mut allowed = Some(PlaybackTarget::parse(FIRST).unwrap());
        assert!(navigation_allowed(allowed.as_ref(), FIRST));
        assert!(navigation_allowed(
            allowed.as_ref(),
            "http://127.0.0.1:32123/abcdefghijklmnop/media"
        ));
        assert!(!navigation_allowed(allowed.as_ref(), SECOND));
        assert!(!navigation_allowed(
            allowed.as_ref(),
            "http://127.0.0.1:32123/abcdefghijklmnop-other/"
        ));
        allowed = Some(PlaybackTarget::parse(SECOND).unwrap());
        assert!(!navigation_allowed(allowed.as_ref(), FIRST));
        assert!(navigation_allowed(allowed.as_ref(), SECOND));
        allowed = None;
        assert!(!navigation_allowed(allowed.as_ref(), FIRST));
        assert!(!navigation_allowed(allowed.as_ref(), SECOND));
        assert!(navigation_allowed(allowed.as_ref(), "about:blank"));
        assert!(!navigation_allowed(
            allowed.as_ref(),
            "about:blank#fragment"
        ));
    }
}
