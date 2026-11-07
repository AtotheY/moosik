use std::{io::Read, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use reqwest::{
    Client, Method,
    header::{HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use tokio::{
    sync::{OnceCell, oneshot, watch},
    task::JoinHandle,
};
use tokio_tungstenite::tungstenite::{
    Message, client::IntoClientRequest, protocol::WebSocketConfig,
};
use url::Url;

use crate::{
    AccountProfile, Error, Result, WebSession, http_client, now_ms,
    web_auth::{cookie_header, validate_web_session},
};

const LIMIT: usize = 16 * 1024 * 1024;
const HARMONY: &str = "harmony:4.80.0-3cdf02c91";

#[derive(Clone)]
pub struct Transport {
    inner: Arc<Inner>,
}

struct Inner {
    http: Client,
    access_token: String,
    device_id: String,
    expires_at: u64,
    headers: HeaderMap,
    user_agent: HeaderValue,
    license_cookie: HeaderValue,
    spclient: Url,
    service_certificate: OnceCell<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct TrackMetadata {
    pub uri: String,
    pub title: String,
    pub artist_names: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub cover_url: Option<String>,
    pub explicit: bool,
}

pub struct PreparedTrack {
    pub metadata: TrackMetadata,
    pub file_id: String,
    signed_url: OnceCell<Url>,
    dealer: Dealer,
}

pub struct RangeData {
    pub bytes: Vec<u8>,
    pub total_len: u64,
}

struct Dealer {
    task: JoinHandle<()>,
}

impl Drop for Dealer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl PreparedTrack {
    pub fn dealer_connected(&self) -> bool {
        !self.dealer.task.is_finished()
    }
}

impl Transport {
    pub async fn account_profile(&self) -> Result<AccountProfile> {
        let response = self
            .query(
                "profileAttributes",
                "08ffb4730af3746e04a8301396f20875dbbce10c75243803091a9274eacc8ac0",
                json!({}),
            )
            .await?;
        let profile = response
            .pointer("/data/me/profile")
            .ok_or(Error::Invalid("account profile"))?;
        crate::model::parse_account_profile(profile)
    }

    pub(crate) async fn query(
        &self,
        operation: &'static str,
        hash: &'static str,
        variables: Value,
    ) -> Result<Value> {
        if self.inner.expires_at <= now_ms()? {
            return Err(Error::AuthorizationExpired);
        }
        let response = self
            .inner
            .http
            .post("https://api-partner.spotify.com/pathfinder/v2/query")
            .headers(self.inner.headers.clone())
            .json(&json!({
                "operationName": operation,
                "variables": variables,
                "extensions": {"persistedQuery": {"version": 1, "sha256Hash": hash}}
            }))
            .send()
            .await
            .map_err(|_| Error::Network(operation))?;
        let response = checked(response, operation)?;
        let bytes = body(response, LIMIT, operation).await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid(operation))?;
        if value["errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty())
        {
            return Err(Error::Invalid(operation));
        }
        Ok(value)
    }

    pub(crate) async fn artist_verified(&self, uri: &str) -> Result<bool> {
        const STAGE: &str = "artist verification";
        if self.inner.expires_at <= now_ms()? {
            return Err(Error::AuthorizationExpired);
        }
        let url = self
            .inner
            .spclient
            .join("/extended-metadata/v0/extended-metadata")
            .map_err(|_| Error::Invalid("API path"))?;
        let response = self
            .inner
            .http
            .post(url)
            .headers(self.inner.headers.clone())
            .header("accept", "application/protobuf")
            .timeout(Duration::from_secs(1))
            .json(&json!({"entityRequest": [{
                "entityUri": uri,
                "query": [{"extensionKind": crate::verification::REPUTATION_EXTENSION, "etag": ""}]
            }]}))
            .send()
            .await
            .map_err(|_| Error::Network(STAGE))?;
        let bytes = body(checked(response, STAGE)?, 16 * 1024, STAGE).await?;
        Ok(crate::verification::verified(&bytes, uri).unwrap_or(false))
    }

    pub async fn build_from_web_session(session: WebSession) -> Result<Self> {
        validate_web_session(&session)?;
        let now = now_ms()?;
        if session.expires_at <= now || session.client_token_expires_at <= now {
            return Err(Error::AuthorizationExpired);
        }
        let http = http_client()?;
        let response = http
            .get("https://apresolve.spotify.com/?type=spclient")
            .send()
            .await
            .map_err(|_| Error::Network("endpoint discovery"))?;
        let response = checked(response, "endpoint discovery")?;
        let bytes = body(response, LIMIT, "endpoint discovery").await?;
        let routes: Value =
            serde_json::from_slice(&bytes).map_err(|_| Error::Invalid("endpoint discovery"))?;
        let spclient = routes["spclient"]
            .as_array()
            .and_then(|routes| routes.iter().filter_map(Value::as_str).find_map(route))
            .ok_or(Error::Invalid("Spotify endpoint"))?;
        let user_agent =
            HeaderValue::from_str(&session.user_agent).map_err(|_| Error::Invalid("user agent"))?;
        let license_cookie = cookie_header(&session.cookie)?;
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("authorization", format!("Bearer {}", session.access_token)),
            ("client-token", session.client_token),
            ("spotify-app-version", session.app_version),
        ] {
            let mut value = HeaderValue::from_str(&value)
                .map_err(|_| Error::Invalid("authorization header"))?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        headers.insert("app-platform", HeaderValue::from_static("WebPlayer"));
        headers.insert(
            "origin",
            HeaderValue::from_static("https://open.spotify.com"),
        );
        headers.insert(
            "referer",
            HeaderValue::from_static("https://open.spotify.com/"),
        );
        headers.insert("user-agent", user_agent.clone());
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                access_token: session.access_token,
                device_id: session.device_id,
                expires_at: session.expires_at.min(session.client_token_expires_at),
                headers,
                user_agent,
                license_cookie,
                spclient,
                service_certificate: OnceCell::new(),
            }),
        })
    }

    pub async fn metadata(&self, uri: &str) -> Result<TrackMetadata> {
        let id = format!("{:032x}", track_id(uri)?);
        let value = self
            .api(
                Method::GET,
                &format!("/metadata/4/track/{id}?alt=json"),
                None,
                "metadata",
            )
            .await?;
        let mut metadata = metadata(&json!({"metadata":value}), uri);
        if metadata.cover_url.is_none() {
            metadata.cover_url = value
                .pointer("/album/cover_group/image/0/file_id")
                .and_then(Value::as_str)
                .filter(|id| id.len() == 40 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .map(|id| format!("https://i.scdn.co/image/{id}"));
        }
        Ok(metadata)
    }

    pub async fn search_tracks(&self, query: &str) -> Result<Vec<TrackMetadata>> {
        let query = query.trim();
        if query.is_empty() || query.len() > 1024 {
            return Err(Error::Invalid("search query"));
        }
        let response = self.inner.http
            .post("https://api-partner.spotify.com/pathfinder/v2/query")
            .headers(self.inner.headers.clone())
            .json(&json!({
                "operationName": "searchTracks",
                "variables": {
                    "searchTerm":query, "offset":0, "limit":10,
                    "includePreReleases":false, "includeAlbumPreReleases":false,
                    "numberOfTopResults":20, "includeAudiobooks":true,
                    "includeAuthors":false, "includeEpisodeContentRatingsV2":false
                },
                "extensions":{"persistedQuery":{"version":1,"sha256Hash":"b02683192a98dde7966b5e6655a79eeb62713eab703eda9902c932818dd52751"}}
            }))
            .send().await.map_err(|_| Error::Network("search"))?;
        let response = checked(response, "search")?;
        let bytes = body(response, 2 * 1024 * 1024, "search").await?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| Error::Invalid("search response"))?;
        search_tracks(&value)
    }

    pub async fn prepare(&self, uri: &str) -> Result<PreparedTrack> {
        let mut track = self.prepare_for_playback(uri).await?;
        self.storage_url(&track).await?;
        track.metadata = self.metadata(uri).await.unwrap_or(track.metadata);
        Ok(track)
    }

    pub async fn prepare_for_playback(&self, uri: &str) -> Result<PreparedTrack> {
        track_id(uri)?;
        if self.inner.expires_at <= now_ms()? {
            return Err(Error::AuthorizationExpired);
        }
        let (dealer, connection_id, mut manifests) = self.connect_dealer(uri).await?;
        let device = &self.inner.device_id;
        self.api(Method::POST, "/track-playback/v1/devices", Some(json!({
            "device": {
                "brand":"spotify", "device_id":device, "device_type":"computer", "metadata":{},
                "model":"web_player", "name":"Moosik", "is_group":false, "is_public":false,
                "platform_identifier":"web_player windows 10.0.0;chrome 156.0.0.0;desktop",
                "correlation_id":correlation(), "client_version":HARMONY,
                "capabilities": {
                    "change_volume":true, "enable_play_token":true, "supports_file_media_type":true,
                    "play_token_lost_behavior":"pause", "disable_connect":false, "audio_podcasts":true,
                    "video_playback":false, "manifest_formats":["file_ids_mp4","file_ids_mp4_dual"],
                    "supports_preferred_media_type":true, "supports_playback_offsets":true, "supports_playback_speed":true
                }
            },
            "connection_id":connection_id, "client_version":HARMONY, "volume":65535,
            "outro_endcontent_snooping":false
        })), "device registration").await?;
        self.api(Method::POST, &format!("/connect-state/v1/player/command/from/{device}/to/{device}"), Some(json!({
            "command": {
                "context":{"uri":uri,"url":format!("context://{uri}"),"metadata":{}},
                "play_origin":{"feature_identifier":"search","feature_version":"web-player_2026-10-09_1791504000000_d873ee9db135","referrer_identifier":"search"},
                "options":{"license":"tft","skip_to":{},"player_options_override":{}},
                "logging_params":{"page_instance_ids":[],"interaction_ids":[],"command_id":correlation()},
                "endpoint":"play"
            }
        })), "play request").await?;
        let resolved = tokio::time::timeout(Duration::from_secs(18), async {
            loop {
                if let Some(value) = manifests.borrow_and_update().clone() {
                    return Ok(value);
                }
                manifests.changed().await.map_err(|_| Error::DealerClosed)?;
            }
        })
        .await
        .map_err(|_| Error::Timeout("manifest"))??;
        let file_id = resolved
            .pointer("/manifest/file_ids_mp4/0/file_id")
            .and_then(Value::as_str)
            .filter(|id| id.len() == 40 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or(Error::Invalid("audio manifest"))?
            .to_owned();
        Ok(PreparedTrack {
            metadata: metadata(&resolved, uri),
            file_id,
            signed_url: OnceCell::new(),
            dealer,
        })
    }

    async fn storage_url<'a>(&self, track: &'a PreparedTrack) -> Result<&'a Url> {
        track
            .signed_url
            .get_or_try_init(|| async {
                if self.inner.expires_at <= now_ms()? {
                    return Err(Error::AuthorizationExpired);
                }
                let file_id = &track.file_id;
                if file_id.len() != 40 || !file_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(Error::Invalid("audio file"));
                }
                let storage = self.api(Method::GET,
                    &format!("/storage-resolve/v2/files/audio/interactive/10/{file_id}?version=10000000&product=9&platform=39&alt=json"),
                    None, "storage resolution").await?;
                storage
                    .pointer("/cdnurl/0")
                    .and_then(Value::as_str)
                    .and_then(|value| Url::parse(value).ok())
                    .filter(valid_cdn)
                    .ok_or(Error::Invalid("audio CDN"))
            })
            .await
    }

    pub async fn fetch_range(
        &self,
        track: &PreparedTrack,
        start: u64,
        end: u64,
    ) -> Result<RangeData> {
        let length = end
            .checked_sub(start)
            .and_then(|length| length.checked_add(1))
            .filter(|length| *length <= LIMIT as u64)
            .ok_or(Error::Invalid("audio range"))?;
        let signed_url = self.storage_url(track).await?;
        if !valid_cdn(signed_url) {
            return Err(Error::Invalid("audio CDN"));
        }
        let response = self
            .inner
            .http
            .get(signed_url.clone())
            .header("range", format!("bytes={start}-{end}"))
            .header("user-agent", self.inner.user_agent.clone())
            .send()
            .await
            .map_err(|_| Error::Network("audio range"))?;
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(Error::Http {
                stage: "audio range",
                status: response.status().as_u16(),
            });
        }
        let content_range = response
            .headers()
            .get("content-range")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("bytes "))
            .and_then(|value| value.split_once('/'))
            .ok_or(Error::Invalid("audio content range"))?;
        let (positions, total) = content_range;
        let (returned_start, returned_end) = positions
            .split_once('-')
            .ok_or(Error::Invalid("audio content range"))?;
        let returned_start: u64 = returned_start
            .parse()
            .map_err(|_| Error::Invalid("audio content range"))?;
        let returned_end: u64 = returned_end
            .parse()
            .map_err(|_| Error::Invalid("audio content range"))?;
        if returned_start != start || returned_end < start || returned_end > end {
            return Err(Error::Invalid("audio content range"));
        }
        let expected = returned_end - returned_start + 1;
        let total_len = total
            .parse()
            .map_err(|_| Error::Invalid("audio total length"))?;
        let bytes = body(response, length as usize, "audio range").await?;
        if bytes.len() as u64 != expected {
            return Err(Error::Invalid("audio range length"));
        }
        Ok(RangeData { bytes, total_len })
    }

    pub async fn license(&self, challenge: &[u8]) -> Result<Vec<u8>> {
        if challenge.is_empty() || challenge.len() > LIMIT {
            return Err(Error::Invalid("license challenge"));
        }
        let url = license_endpoint(&self.inner.spclient)?;
        let request = self
            .inner
            .http
            .post(url)
            .headers(self.inner.headers.clone())
            .header("cookie", self.inner.license_cookie.clone())
            .header("content-type", "application/octet-stream")
            .body(challenge.to_vec());
        let response = request
            .send()
            .await
            .map_err(|_| Error::Network("license"))?;
        body(checked(response, "license")?, LIMIT, "license").await
    }

    pub async fn service_certificate(&self) -> Result<Vec<u8>> {
        self.cached_service_certificate(|| self.license(&[0x08, 0x04]))
            .await
    }

    async fn cached_service_certificate<F, Fut>(&self, fetch: F) -> Result<Vec<u8>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>>>,
    {
        if self.inner.expires_at <= now_ms()? {
            return Err(Error::AuthorizationExpired);
        }
        Ok(self
            .inner
            .service_certificate
            .get_or_try_init(|| async {
                let bytes = fetch().await?;
                if !valid_service_certificate(&bytes) {
                    return Err(Error::Invalid("service certificate"));
                }
                Ok(bytes)
            })
            .await?
            .clone())
    }

    async fn api(
        &self,
        method: Method,
        path: &str,
        payload: Option<Value>,
        stage: &'static str,
    ) -> Result<Value> {
        let url = self
            .inner
            .spclient
            .join(path)
            .map_err(|_| Error::Invalid("API path"))?;
        let mut request = self
            .inner
            .http
            .request(method, url)
            .headers(self.inner.headers.clone());
        if let Some(payload) = payload {
            request = request.json(&payload);
        }
        let response = request.send().await.map_err(|_| Error::Network(stage))?;
        let bytes = body(checked(response, stage)?, LIMIT, stage).await?;
        if bytes.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::Invalid(stage))
    }

    async fn connect_dealer(
        &self,
        uri: &str,
    ) -> Result<(Dealer, String, watch::Receiver<Option<Value>>)> {
        let mut url =
            Url::parse("wss://dealer.spotify.com/").map_err(|_| Error::Invalid("Dealer URL"))?;
        url.query_pairs_mut()
            .append_pair("access_token", &self.inner.access_token);
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|_| Error::Invalid("Dealer request"))?;
        request.headers_mut().insert(
            "origin",
            HeaderValue::from_static("https://open.spotify.com"),
        );
        request
            .headers_mut()
            .insert("user-agent", self.inner.user_agent.clone());
        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(LIMIT);
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(12),
            tokio_tungstenite::connect_async_with_config(request, Some(config), false),
        )
        .await
        .map_err(|_| Error::Timeout("Dealer connect"))?
        .map_err(|_| Error::Network("Dealer connect"))?;
        let (connection_tx, connection_rx) = oneshot::channel();
        let (manifest_tx, manifest_rx) = watch::channel(None);
        let wanted = uri.to_owned();
        let task = tokio::spawn(async move {
            let (mut output, mut input) = socket.split();
            let mut connection_tx = Some(connection_tx);
            let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
            heartbeat.tick().await;
            loop {
                tokio::select! {
                    _ = heartbeat.tick() => {
                        if output.send(Message::Text(json!({"type":"ping"}).to_string().into())).await.is_err() { break; }
                    }
                    incoming = input.next() => {
                        let text = match incoming {
                            Some(Ok(Message::Text(text))) => text,
                            Some(Ok(Message::Ping(payload))) => {
                                if output.send(Message::Pong(payload)).await.is_err() { break; }
                                continue;
                            }
                            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                            _ => continue,
                        };
                        let Ok(value) = serde_json::from_str::<Value>(&text) else { continue; };
                        if let Some(id) = value.pointer("/headers/Spotify-Connection-Id").or_else(|| value.pointer("/headers/x-spotify-connection-id")).and_then(Value::as_str)
                            && let Some(sender) = connection_tx.take()
                        {
                            let _ = sender.send(id.to_owned());
                        }
                        let reply = match value["type"].as_str() {
                            Some("ping") => Some(json!({"type":"pong"})),
                            Some("request") => value["key"].as_str().map(|key| json!({"type":"reply","key":key,"payload":{"success":true}})),
                            _ => None,
                        };
                        if let Some(reply) = reply
                            && output.send(Message::Text(reply.to_string().into())).await.is_err()
                        {
                            break;
                        }
                        if let Some(found) = find_manifest(&value, &wanted, 0) { let _ = manifest_tx.send(Some(found.clone())); }
                        if let Some(payloads) = value["payloads"].as_array() {
                            for payload in payloads.iter().filter_map(Value::as_str) {
                                if let Some(payload) = decode_payload(payload)
                                    && let Some(found) = find_manifest(&payload, &wanted, 0)
                                {
                                    let _ = manifest_tx.send(Some(found.clone()));
                                }
                            }
                        }
                    }
                }
            }
        });
        let dealer = Dealer { task };
        let connection = tokio::time::timeout(Duration::from_secs(12), connection_rx)
            .await
            .map_err(|_| Error::Timeout("Dealer connection identity"))?
            .map_err(|_| Error::DealerClosed)?;
        Ok((dealer, connection, manifest_rx))
    }
}

fn valid_service_certificate(bytes: &[u8]) -> bool {
    let Some(encoded) = bytes.strip_prefix(&[8, 5, 18]) else {
        return false;
    };
    let mut length = 0usize;
    for shift in (0..usize::BITS).step_by(7) {
        let at = (shift / 7) as usize;
        let Some(&byte) = encoded.get(at) else {
            return false;
        };
        let group = usize::from(byte & 127);
        if group > usize::MAX >> shift {
            return false;
        }
        length |= group << shift;
        if byte < 128 {
            return length > 0 && length == encoded.len() - at - 1;
        }
    }
    false
}

fn license_endpoint(base: &Url) -> Result<Url> {
    let host = base.host_str().ok_or(Error::Invalid("license endpoint"))?;
    if base.scheme() != "https"
        || !base.username().is_empty()
        || base.password().is_some()
        || base.port_or_known_default() != Some(443)
        || !(host == "spclient.wg.spotify.com" || host.ends_with("-spclient.spotify.com"))
        || base.path() != "/"
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(Error::Invalid("license endpoint"));
    }
    base.join("/widevine-license/v1/audio/license")
        .map_err(|_| Error::Invalid("license endpoint"))
}

fn route(value: &str) -> Option<Url> {
    let url = Url::parse(&format!("https://{value}/")).ok()?;
    let host = url.host_str()?;
    (url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && (host == "spclient.wg.spotify.com" || host.ends_with("-spclient.spotify.com")))
    .then_some(url)
}

fn valid_cdn(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && ["spotifycdn.com", "scdn.co"]
            .iter()
            .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

fn track_id(uri: &str) -> Result<u128> {
    let id = uri
        .strip_prefix("spotify:track:")
        .filter(|id| id.len() == 22)
        .ok_or(Error::Invalid("track URI"))?;
    let mut value = 0u128;
    for byte in id.bytes() {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'z' => byte - b'a' + 10,
            b'A'..=b'Z' => byte - b'A' + 36,
            _ => return Err(Error::Invalid("track URI")),
        };
        value = value
            .checked_mul(62)
            .and_then(|value| value.checked_add(u128::from(digit)))
            .ok_or(Error::Invalid("track URI"))?;
    }
    Ok(value)
}

fn correlation() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn checked(response: reqwest::Response, stage: &'static str) -> Result<reqwest::Response> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(Error::Http {
            stage,
            status: response.status().as_u16(),
        })
    }
}

async fn body(
    mut response: reqwest::Response,
    limit: usize,
    stage: &'static str,
) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Error::Invalid("response length"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network(stage))? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(Error::Invalid("response length"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn find_manifest<'a>(value: &'a Value, uri: &str, depth: usize) -> Option<&'a Value> {
    if depth > 14 {
        return None;
    }
    if value.pointer("/metadata/uri").and_then(Value::as_str) == Some(uri)
        && value
            .pointer("/manifest/file_ids_mp4")
            .is_some_and(Value::is_array)
    {
        return Some(value);
    }
    match value {
        Value::Object(values) => values
            .values()
            .find_map(|value| find_manifest(value, uri, depth + 1)),
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_manifest(value, uri, depth + 1)),
        _ => None,
    }
}

fn decode_payload(value: &str) -> Option<Value> {
    if value.len() > LIMIT {
        return None;
    }
    if value.trim_start().starts_with('{') {
        return serde_json::from_str(value).ok();
    }
    let decoded = STANDARD.decode(value).ok()?;
    let reader: Box<dyn Read> = if decoded.starts_with(&[31, 139]) {
        Box::new(flate2::read::GzDecoder::new(decoded.as_slice()))
    } else if decoded.first() == Some(&120) {
        Box::new(flate2::read::ZlibDecoder::new(decoded.as_slice()))
    } else {
        Box::new(decoded.as_slice())
    };
    let mut bytes = Vec::new();
    reader.take(LIMIT as u64 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > LIMIT {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn metadata(value: &Value, uri: &str) -> TrackMetadata {
    let metadata = &value["metadata"];
    let string = |key: &str| metadata[key].as_str().map(str::to_owned);
    let artists = metadata["artists"]
        .as_array()
        .or_else(|| metadata["artist"].as_array())
        .map(|artists| {
            artists
                .iter()
                .filter_map(|artist| {
                    artist
                        .as_str()
                        .or_else(|| artist["name"].as_str())
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    TrackMetadata {
        uri: uri.to_owned(),
        title: string("name")
            .or_else(|| string("title"))
            .unwrap_or_else(|| uri.to_owned()),
        artist_names: artists,
        album: string("album_name").or_else(|| {
            metadata
                .pointer("/album/name")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }),
        duration_ms: metadata["duration_ms"]
            .as_u64()
            .or_else(|| metadata["duration"].as_u64()),
        cover_url: string("image_url").or_else(|| string("cover_url")),
        explicit: metadata["explicit"].as_bool().unwrap_or(false),
    }
}

fn search_tracks(value: &Value) -> Result<Vec<TrackMetadata>> {
    if value["errors"]
        .as_array()
        .is_some_and(|errors| !errors.is_empty())
    {
        return Err(Error::Invalid("search response"));
    }
    let items = value
        .pointer("/data/searchV2/tracksV2/items")
        .and_then(Value::as_array)
        .ok_or(Error::Invalid("search results"))?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let track = item.pointer("/item/data")?;
            if track["__typename"].as_str() != Some("Track")
                || track
                    .pointer("/playability/playable")
                    .and_then(Value::as_bool)
                    == Some(false)
            {
                return None;
            }
            let uri = track["uri"].as_str()?;
            track_id(uri).ok()?;
            let title = track["name"].as_str()?.to_owned();
            if title.is_empty() {
                return None;
            }
            let artist_names = track
                .pointer("/artists/items")
                .and_then(Value::as_array)
                .map(|artists| {
                    artists
                        .iter()
                        .filter_map(|artist| {
                            artist
                                .pointer("/profile/name")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let cover_url = track
                .pointer("/albumOfTrack/coverArt/sources")
                .and_then(Value::as_array)
                .and_then(|images| {
                    images
                        .iter()
                        .filter(|image| {
                            image["url"]
                                .as_str()
                                .is_some_and(|url| url.starts_with("https://"))
                        })
                        .max_by_key(|image| image["width"].as_u64().unwrap_or(0))
                })
                .and_then(|image| image["url"].as_str())
                .map(str::to_owned);
            Some(TrackMetadata {
                uri: uri.to_owned(),
                title,
                artist_names,
                album: track
                    .pointer("/albumOfTrack/name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                duration_ms: track
                    .pointer("/duration/totalMilliseconds")
                    .and_then(Value::as_u64),
                cover_url,
                explicit: track
                    .pointer("/contentRating/label")
                    .and_then(Value::as_str)
                    == Some("EXPLICIT"),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport(expires_at: u64) -> Transport {
        Transport {
            inner: Arc::new(Inner {
                http: http_client().unwrap(),
                access_token: String::new(),
                device_id: String::new(),
                expires_at,
                headers: HeaderMap::new(),
                user_agent: HeaderValue::from_static("Moosik test"),
                license_cookie: HeaderValue::from_static("sp_dc=test"),
                spclient: Url::parse("http://127.0.0.1/").unwrap(),
                service_certificate: OnceCell::new(),
            }),
        }
    }

    fn prepared_track(file_id: &str) -> PreparedTrack {
        PreparedTrack {
            metadata: TrackMetadata {
                uri: "spotify:track:0000000000000000000000".to_owned(),
                title: String::new(),
                artist_names: Vec::new(),
                album: None,
                duration_ms: None,
                cover_url: None,
                explicit: false,
            },
            file_id: file_id.to_owned(),
            signed_url: OnceCell::new(),
            dealer: Dealer {
                task: tokio::spawn(std::future::pending()),
            },
        }
    }

    #[test]
    fn service_certificate_cache_accepts_only_complete_certificates() {
        assert!(valid_service_certificate(&[8, 5, 18, 3, 1, 2, 3]));
        let mut long = vec![8, 5, 18, 128, 1];
        long.extend([1; 128]);
        assert!(valid_service_certificate(&long));
        for invalid in [
            &[8, 4, 18, 3, 1, 2, 3][..],
            &[8, 5, 18, 4, 1, 2, 3],
            &[8, 5, 18, 2, 1, 2, 3],
            &[8, 5, 18, 0],
            &[8, 5, 18, 128],
            &[8, 5, 18, 255, 255, 255, 255, 255, 255, 255, 255, 255, 127],
        ] {
            assert!(!valid_service_certificate(invalid));
        }
    }

    #[tokio::test]
    async fn transport_clones_share_one_certificate_fetch() {
        let transport = transport(u64::MAX);
        let bytes = vec![8, 5, 18, 3, 1, 2, 3];
        let cloned = transport.clone();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let fetch = || async {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(bytes.clone())
        };
        let (first, second) = tokio::join!(
            transport.cached_service_certificate(fetch),
            cloned.cached_service_certificate(fetch)
        );
        assert_eq!(first.unwrap(), bytes);
        assert_eq!(second.unwrap(), bytes);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(transport.service_certificate().await.unwrap(), bytes);
    }

    #[tokio::test]
    async fn failed_certificate_loads_leave_the_cache_retryable() {
        let transport = transport(u64::MAX);
        assert!(matches!(
            transport
                .cached_service_certificate(|| async { Err(Error::Network("license")) })
                .await,
            Err(Error::Network("license"))
        ));
        assert!(transport.inner.service_certificate.get().is_none());
        assert!(matches!(
            transport
                .cached_service_certificate(|| async { Ok(vec![8, 5, 18, 4, 1, 2, 3]) })
                .await,
            Err(Error::Invalid("service certificate"))
        ));
        assert!(transport.inner.service_certificate.get().is_none());
        let bytes = vec![8, 5, 18, 3, 1, 2, 3];
        assert_eq!(
            transport
                .cached_service_certificate(|| async { Ok(bytes.clone()) })
                .await
                .unwrap(),
            bytes
        );
    }

    #[tokio::test]
    async fn expired_transport_rejects_new_playback_and_certificate_loads() {
        let transport = transport(0);
        transport
            .inner
            .service_certificate
            .set(vec![8, 5, 18, 3, 1, 2, 3])
            .unwrap();
        let uri = "spotify:track:0000000000000000000000";
        assert!(matches!(
            transport.prepare_for_playback(uri).await,
            Err(Error::AuthorizationExpired)
        ));
        assert!(matches!(
            transport.prepare(uri).await,
            Err(Error::AuthorizationExpired)
        ));
        assert!(matches!(
            transport.service_certificate().await,
            Err(Error::AuthorizationExpired)
        ));
    }

    #[tokio::test]
    async fn lazy_storage_retries_failures_and_retains_resolved_urls() {
        let mut transport = transport(0);
        let track = prepared_track("invalid");
        assert!(matches!(
            transport.storage_url(&track).await,
            Err(Error::AuthorizationExpired)
        ));
        assert!(track.signed_url.get().is_none());
        Arc::get_mut(&mut transport.inner).unwrap().expires_at = u64::MAX;
        assert!(matches!(
            transport.storage_url(&track).await,
            Err(Error::Invalid("audio file"))
        ));
        assert!(track.signed_url.get().is_none());
        let url = Url::parse("https://audio.spotifycdn.com/test").unwrap();
        track.signed_url.set(url.clone()).unwrap();
        Arc::get_mut(&mut transport.inner).unwrap().expires_at = 0;
        assert_eq!(transport.storage_url(&track).await.unwrap(), &url);
    }

    #[test]
    fn search_preserves_source_metadata_and_rejects_failed_responses() {
        let track = json!({"__typename":"Track","uri":"spotify:track:0FIDCNYYjNvPVimz5icugS","name":"Timeless (feat Playboi Carti)","artists":{"items":[{"profile":{"name":"The Weeknd"}},{"profile":{"name":"Playboi Carti"}}]},"albumOfTrack":{"name":"Hurry Up Tomorrow","coverArt":{"sources":[{"url":"https://i.scdn.co/image/small","width":64},{"url":"https://i.scdn.co/image/large","width":640}]}},"duration":{"totalMilliseconds":256000},"contentRating":{"label":"EXPLICIT"},"playability":{"playable":true}});
        let mut unavailable = track.clone();
        unavailable["playability"]["playable"] = json!(false);
        let mut invalid = track.clone();
        invalid["uri"] = json!("spotify:album:0FIDCNYYjNvPVimz5icugS");
        let response = json!({"data":{"searchV2":{"tracksV2":{"items":[{"item":{"data":track}},{"item":{"data":unavailable}},{"item":{"data":invalid}},{"item":{"data":{"__typename":"NotFound"}}}]}}}});
        let result = search_tracks(&response).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].uri, "spotify:track:0FIDCNYYjNvPVimz5icugS");
        assert_eq!(result[0].artist_names, ["The Weeknd", "Playboi Carti"]);
        assert_eq!(result[0].duration_ms, Some(256000));
        assert_eq!(
            result[0].cover_url.as_deref(),
            Some("https://i.scdn.co/image/large")
        );
        assert!(result[0].explicit);
        assert!(search_tracks(&json!({"errors":[{"message":"denied"}]})).is_err());
        assert!(search_tracks(&json!({"data":{}})).is_err());
    }

    #[test]
    fn license_cookies_stay_on_allowed_spotify_endpoints() {
        let base = Url::parse("https://gae2-spclient.spotify.com/").unwrap();
        let license = license_endpoint(&base).unwrap();
        assert_eq!(license.host_str(), base.host_str());
        assert_eq!(license.path(), "/widevine-license/v1/audio/license");
        for invalid in [
            "https://gae2-spclient.spotify.com.evil.test/",
            "https://open.spotify.com/",
            "http://gae2-spclient.spotify.com/",
            "https://user@gae2-spclient.spotify.com/",
            "https://gae2-spclient.spotify.com/other",
            "https://gae2-spclient.spotify.com/?other=value",
        ] {
            assert!(license_endpoint(&Url::parse(invalid).unwrap()).is_err());
        }
    }
    #[test]
    fn base62_tracks_preserve_spotify_digit_order() {
        assert_eq!(
            format!(
                "{:032x}",
                track_id("spotify:track:3TO7bbrUKrOSPGRTB5MeCz").unwrap()
            ),
            "802569d2b78147838c4a39a20741b0ff"
        );
        assert_eq!(
            track_id("spotify:track:000000000000000000000a").unwrap(),
            10
        );
        assert_eq!(
            track_id("spotify:track:000000000000000000000A").unwrap(),
            36
        );
        assert_eq!(
            format!(
                "{:032x}",
                track_id("spotify:track:0000000000000000000000").unwrap()
            ),
            "00000000000000000000000000000000"
        );
    }

    #[test]
    fn base62_tracks_reject_overflow_and_invalid_ids() {
        assert_eq!(
            track_id("spotify:track:7N42dgm5tFLK9N8MT7fHC7").unwrap(),
            u128::MAX
        );
        for invalid in [
            "spotify:track:7N42dgm5tFLK9N8MT7fHC8",
            "spotify:track:ZZZZZZZZZZZZZZZZZZZZZZ",
            "spotify:track:000000000000000000000!",
            "spotify:track:short",
            "spotify:album:3TO7bbrUKrOSPGRTB5MeCz",
        ] {
            assert!(track_id(invalid).is_err());
        }
    }
}
