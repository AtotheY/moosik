//! Boundaries for requests carrying the YouTube session. Never log rejected URLs.
use anyhow::{Result, bail};
use url::Url;

pub(crate) fn tracking_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| anyhow::anyhow!("invalid playback reporting URL"))?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("www.youtube.com" | "music.youtube.com" | "youtube.com")
        )
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "/api/stats/playback" | "/api/stats/watchtime")
    {
        bail!("blocked an untrusted playback reporting URL");
    }
    Ok(url)
}

pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .use_rustls_tls()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_actual_youtube_reporting_endpoints() {
        for host in ["www.youtube.com", "music.youtube.com", "youtube.com"] {
            for path in ["playback", "watchtime"] {
                assert!(tracking_url(&format!("https://{host}/api/stats/{path}?v=dummy")).is_ok());
            }
        }
    }
    #[test]
    fn rejects_credential_exfiltration_destinations() {
        for url in [
            "http://www.youtube.com/api/stats/watchtime",
            "https://youtube.com.evil.test/api/stats/watchtime",
            "https://evilyoutube.com/api/stats/watchtime",
            "https://www.youtube.com@evil.test/api/stats/watchtime",
            "https://evil.test@www.youtube.com/api/stats/watchtime",
            "https://www.youtube.com:444/api/stats/watchtime",
            "https://127.0.0.1/api/stats/watchtime",
            "https://www.youtube.com/redirect?to=evil.test",
            "https://www.youtube.com/api/stats/watchtime#fragment",
            "https://www.youtube.com./api/stats/watchtime",
            "https://www.youtube.com/api/stats/%77atchtime",
            "file:///api/stats/watchtime",
        ] {
            assert!(tracking_url(url).is_err(), "accepted {url}");
        }
    }
    #[tokio::test]
    async fn credential_client_rejects_http_before_network_access() {
        let client = client_builder().build().unwrap();
        let err = client
            .get("http://127.0.0.1:9/")
            .header("Cookie", "dummy")
            .send()
            .await
            .unwrap_err();
        assert!(err.is_builder());
    }
    #[tokio::test]
    async fn credential_client_does_not_follow_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).contains("dummy-session"));
            socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        // Allow HTTP only inside this dummy loopback test; preserve the production redirect policy.
        let client = client_builder().https_only(false).build().unwrap();
        let response = client
            .get(format!("http://{addr}/start"))
            .header("Cookie", "dummy-session")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        assert_eq!(response.url().path(), "/start");
        server.await.unwrap();
    }
}
