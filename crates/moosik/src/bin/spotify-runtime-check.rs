#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener};
    use std::time::{Duration, Instant};

    use anyhow::{Context, ensure};
    use tao::event::Event;
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::platform::windows::WindowBuilderExtWindows;
    use tao::window::WindowBuilder;
    use wry::{WebContext, WebViewBuilder};

    const HTML: &str = r#"<!doctype html><meta charset="utf-8"><script>
    (async () => {
      const result = {
        secureContext: isSecureContext,
        mediaSource: typeof MediaSource === 'function' && MediaSource.isTypeSupported('audio/mp4; codecs="mp4a.40.2"'),
        encryptedMedia: typeof navigator.requestMediaKeySystemAccess === 'function',
      };
      for (const [name, system] of [['widevine', 'com.widevine.alpha'], ['playready', 'com.microsoft.playready.recommendation']]) {
        try {
          await navigator.requestMediaKeySystemAccess(system, [{
            initDataTypes: ['cenc'],
            audioCapabilities: [{contentType: 'audio/mp4; codecs="mp4a.40.2"'}],
            distinctiveIdentifier: 'optional',
            persistentState: 'optional',
            sessionTypes: ['temporary'],
          }]);
          result[name] = {supported: true};
        } catch (error) {
          const names = ['NotSupportedError', 'SecurityError', 'TypeError'];
          result[name] = {supported: false, error: names.includes(error.name) ? error.name : 'Error'};
        }
      }
      window.ipc.postMessage(JSON.stringify(result));
    })();
    </script>"#;

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = format!("http://127.0.0.1:{}/", listener.local_addr()?.port());
    std::thread::spawn(move || {
        let Ok((mut socket, _)) = listener.accept() else {
            return;
        };
        let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
            if socket.read_exact(&mut byte).is_err() {
                return;
            }
            request.push(byte[0]);
        }
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'\r\n\r\n",
            HTML.len()
        );
        let _ = socket.write_all(header.as_bytes());
        let _ = socket.write_all(HTML.as_bytes());
    });

    let temporary_root = std::env::temp_dir().canonicalize()?;
    let profile = temporary_root.join(format!(
        "moosik-runtime-check-{:032x}",
        rand::random::<u128>()
    ));
    std::fs::create_dir(&profile)?;
    let mut event_loop = EventLoopBuilder::<String>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_visible(false)
        .with_skip_taskbar(true)
        .build(&event_loop)?;
    let mut context = WebContext::new(Some(profile.clone()));
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(&address)
        .with_ipc_handler(move |request| {
            let _ = proxy.send_event(request.body().clone());
        })
        .with_devtools(false)
        .build(&window)
        .context("Could not create a diagnostic WebView2 renderer")?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut result = None;
    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(deadline);
        if let Event::UserEvent(value) = event {
            result = serde_json::from_str::<serde_json::Value>(&value).ok();
            *control_flow = ControlFlow::Exit;
        } else if Instant::now() >= deadline {
            *control_flow = ControlFlow::Exit;
        }
    });
    drop(webview);
    drop(context);
    drop(window);
    drop(event_loop);
    ensure!(
        profile.parent() == Some(temporary_root.as_path()),
        "Diagnostic profile is outside temporary storage"
    );
    let _ = std::fs::remove_dir_all(&profile);
    let result = result.context("WebView2 capability check timed out")?;
    println!("WebView2 {}", wry::webview_version()?);
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("The Spotify playback runtime check requires Windows WebView2")
}
