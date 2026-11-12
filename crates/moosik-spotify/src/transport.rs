use std::{
    io::Read,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use reqwest::{
    Client, Method,
    header::{HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, OnceCell, oneshot, watch},
    task::JoinHandle,
};
use tokio_tungstenite::tungstenite::{
    Message, client::IntoClientRequest, protocol::WebSocketConfig,
};
use url::Url;

use crate::{
    AccountProfile, ArtistCredit, CatalogTrack, Error, Result, WebSession, http_client, now_ms,
    web_auth::{cookie_header, validate_web_session},
};

const LIMIT: usize = 16 * 1024 * 1024;
const HARMONY: &str = "harmony:4.80.0-3cdf02c91";
const PLATFORM: &str = "web_player windows 10.0.0;chrome 156.0.0.0;desktop";
const MEDIA_REPORT_PLATFORM: &str = "web_player windows 10;chrome 156.0.0.0;desktop";
const REPORT_TIMEOUT: Duration = Duration::from_secs(2);
const REPORT_DIAGNOSTIC_LIMIT: usize = 2 * 1024;

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
    playback_registration: Mutex<u64>,
    playback_session: String,
    verification_sequence: AtomicU64,
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
    reporting: Option<Mutex<ReportingState>>,
    verification: Option<StreamVerification>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackReportKind {
    Loading,
    Started,
    Paused,
    Resumed,
    Seeked,
    ThresholdReached,
    Finalized,
}

impl PlaybackReportKind {
    fn debug_source(self) -> &'static str {
        match self {
            Self::Loading => "before_track_load",
            Self::Started => "started_playing",
            Self::Paused => "pause",
            Self::Resumed => "resume",
            Self::Seeked => "seek",
            Self::ThresholdReached => "played_threshold_reached",
            Self::Finalized => "track_data_finalized",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PlaybackReport {
    pub kind: PlaybackReportKind,
    pub position_ms: u64,
    pub previous_position_ms: Option<u64>,
    pub duration_ms: u64,
    pub played_ms: u64,
    pub start_offset_ms: u64,
    pub paused: bool,
    pub playback_speed: f64,
    pub timeline: Option<PlaybackTimeline>,
}

#[derive(Clone, Copy, Debug)]
pub struct PlaybackTimeline {
    pub loaded_at_ms: u64,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
}

impl PlaybackTimeline {
    fn is_ordered(&self) -> bool {
        self.loaded_at_ms > 0
            && self.loaded_at_ms <= self.started_at_ms
            && self.started_at_ms < self.ended_at_ms
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlaybackReference {
    state_machine_id: String,
    state_id: String,
}

#[derive(Clone)]
struct SelectedPlayback {
    reference: PlaybackReference,
    file_id: String,
}

struct StreamVerification {
    playback_id: String,
    internal_play_id: u64,
    bitrate: Option<u64>,
    sequence: OnceLock<u64>,
    sent: AtomicBool,
    stats_sent: AtomicBool,
    media_sent: AtomicBool,
    start_sent: AtomicBool,
    initially_paused: OnceLock<bool>,
    playback_media: OnceLock<PlaybackMedia>,
}

struct PlaybackMedia {
    total_bytes: u64,
    persistent_cached: bool,
}

impl StreamVerification {
    fn new(playback_id: String, internal_play_id: u64, bitrate: Option<u64>) -> Self {
        Self {
            playback_id,
            internal_play_id,
            bitrate,
            sequence: OnceLock::new(),
            sent: AtomicBool::new(false),
            stats_sent: AtomicBool::new(false),
            media_sent: AtomicBool::new(false),
            start_sent: AtomicBool::new(false),
            initially_paused: OnceLock::new(),
            playback_media: OnceLock::new(),
        }
    }

    fn sequence(&self, next: &AtomicU64) -> u64 {
        *self
            .sequence
            .get_or_init(|| next.fetch_add(1, Ordering::Relaxed))
    }
}

struct ReportingState {
    registration: u64,
    sequence: u64,
    updated_reference: Option<(PlaybackReference, PlaybackReference)>,
    closed: bool,
    bitrate: Option<u64>,
    format: Option<u64>,
    audio_quality: Option<String>,
}

impl ReportingState {
    fn payload(
        &mut self,
        reference: &PlaybackReference,
        metadata_duration_ms: Option<u64>,
        report: PlaybackReport,
    ) -> Result<Value> {
        let stationary = report.paused || report.kind == PlaybackReportKind::Loading;
        if !stationary && (!report.playback_speed.is_finite() || report.playback_speed <= 0.0) {
            return Err(Error::Invalid("playback speed"));
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(Error::Invalid("playback sequence"))?;
        let mut payload = json!({
            "seq_num": self.sequence,
            "state_ref": {
                "state_machine_id": reference.state_machine_id,
                "state_id": reference.state_id,
                "paused": report.paused,
            },
            "sub_state": {
                "playback_speed": if stationary { 0.0 } else { report.playback_speed },
                "position": report.position_ms,
                "media_type": "AUDIO",
                "is_video_on": false,
            },
            "debug_source": report.kind.debug_source(),
        });
        let sub_state = &mut payload["sub_state"];
        if report.duration_ms > 0 {
            sub_state["duration"] = json!(report.duration_ms);
        }
        if let Some(bitrate) = self.bitrate {
            sub_state["bitrate"] = json!(bitrate);
        }
        if let Some(format) = self.format {
            sub_state["format"] = json!(format);
        }
        if let Some(quality) = &self.audio_quality {
            sub_state["audio_quality"] = json!(quality);
        }
        let previous_position = match report.kind {
            PlaybackReportKind::Started
            | PlaybackReportKind::Paused
            | PlaybackReportKind::Resumed
            | PlaybackReportKind::ThresholdReached => Some(report.position_ms),
            _ => report.previous_position_ms,
        };
        if let Some(previous_position) = previous_position {
            payload["previous_position"] = json!(previous_position);
        }
        if report.kind == PlaybackReportKind::Finalized {
            const CODEC: &str = "mp4a.40.2";
            let mut stats = json!({
                "ms_total_est": report.duration_ms,
                "start_offset_ms": report.start_offset_ms,
                "audiocodec": CODEC,
                "key_system": "widevine",
            });
            if let Some(duration) = metadata_duration_ms {
                stats["ms_metadata_duration"] = json!(duration);
            }
            if let Some(bitrate) = self.bitrate.filter(|bitrate| *bitrate > 0) {
                let format = format!("{CODEC};{bitrate}");
                stats["ms_played_per_audio_format"] = json!({ (format): report.played_ms });
            }
            payload["playback_stats"] = stats;
        }
        Ok(payload)
    }
}

pub struct RangeData {
    pub bytes: Vec<u8>,
    pub total_len: u64,
}

struct Dealer {
    task: JoinHandle<()>,
    selected: watch::Receiver<Option<SelectedPlayback>>,
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

    pub fn record_playback_media(&self, total_bytes: u64, persistent_cached: bool) {
        if let Some(verification) = &self.verification {
            let _ = verification.playback_media.set(PlaybackMedia {
                total_bytes: if persistent_cached { 0 } else { total_bytes },
                persistent_cached,
            });
        }
    }

    pub fn can_report_playback(&self) -> bool {
        self.reporting.is_some()
            && self
                .dealer
                .selected
                .borrow()
                .as_ref()
                .is_some_and(|selection| selection.file_id == self.file_id)
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

    pub(crate) async fn mutate(
        &self,
        operation: &'static str,
        hash: &'static str,
        variables: Value,
    ) -> Result<()> {
        let response = self.query(operation, hash, variables).await?;
        if !response["data"]
            .as_object()
            .is_some_and(|data| !data.is_empty())
        {
            return Err(Error::Invalid(operation));
        }
        Ok(())
    }

    pub async fn set_track_liked(&self, uri: &str, liked: bool) -> Result<()> {
        track_id(uri)?;
        let (operation, variables) = if liked {
            (
                "addToLibrary",
                json!({"libraryItemUris": [uri], "interactionId": null}),
            )
        } else {
            ("removeFromLibrary", json!({"libraryItemUris": [uri]}))
        };
        self.mutate(
            operation,
            "896ebcb47815681340860d121cb5d494e157e2a78d3950385cd54e0393c67148",
            variables,
        )
        .await
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
                playback_registration: Mutex::new(0),
                playback_session: now_ms()?.to_string(),
                verification_sequence: AtomicU64::new(0),
            }),
        })
    }

    pub async fn metadata(&self, uri: &str) -> Result<TrackMetadata> {
        let value = self.track_metadata(uri).await?;
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

    pub async fn autoplay_next(
        &self,
        seed: &str,
        recent: &[String],
    ) -> Result<Option<CatalogTrack>> {
        const STAGE: &str = "Spotify autoplay";
        track_id(seed)?;
        if self.inner.expires_at <= now_ms()? {
            return Err(Error::AuthorizationExpired);
        }
        let previous = recent
            .iter()
            .filter(|uri| track_id(uri).is_ok())
            .filter_map(|uri| uri.strip_prefix("spotify:track:"))
            .collect::<Vec<_>>()
            .join(",");
        let response = self
            .api(
                Method::GET,
                &format!(
                    "/radio-apollo/v3/tracks/{seed}?autoplay=true&count=12&prev_tracks={previous}"
                ),
                None,
                STAGE,
            )
            .await?;
        autoplay_track(&response, seed, recent)
    }

    pub(crate) async fn canonical_track_uri(&self, uri: &str) -> Option<String> {
        let metadata = self.track_metadata(uri).await.ok()?;
        let canonical = metadata["canonical_uri"].as_str()?;
        track_id(canonical).ok()?;
        Some(canonical.to_owned())
    }

    async fn track_metadata(&self, uri: &str) -> Result<Value> {
        let id = format!("{:032x}", track_id(uri)?);
        self.api(
            Method::GET,
            &format!("/metadata/4/track/{id}?alt=json"),
            None,
            "metadata",
        )
        .await
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
        let mut registration = self.inner.playback_registration.lock().await;
        *registration = registration.wrapping_add(1);
        let registration_id = *registration;
        let registered = self.api(Method::POST, "/track-playback/v1/devices", Some(json!({
            "device": {
                "brand":"spotify", "device_id":device, "device_type":"computer", "metadata":{},
                "model":"web_player", "name":"Moosik", "is_group":false, "is_public":false,
                "platform_identifier":PLATFORM,
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
        drop(registration);
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
        let file = &resolved["manifest"]["file_ids_mp4"][0];
        let file_id = audio_file_id(file)
            .ok_or(Error::Invalid("audio manifest"))?
            .to_owned();
        let bitrate = unsigned_number(&file["bitrate"]).or_else(|| {
            file.get("bitrate")
                .is_none_or(Value::is_null)
                .then_some(128_000)
        });
        let reporting = unsigned_number(&registered["initial_seq_num"]).map(|sequence| {
            Mutex::new(ReportingState {
                registration: registration_id,
                sequence,
                updated_reference: None,
                closed: false,
                bitrate,
                format: unsigned_number(&file["format"]),
                audio_quality: file["audio_quality"].as_str().map(str::to_owned),
            })
        });
        let verification = dealer
            .selected
            .borrow()
            .as_ref()
            .filter(|selection| selection.file_id == file_id)
            .map(|selection| {
                StreamVerification::new(
                    selection.reference.state_id.clone(),
                    registration_id,
                    bitrate,
                )
            });
        Ok(PreparedTrack {
            metadata: metadata(&resolved, uri),
            file_id,
            signed_url: OnceCell::new(),
            dealer,
            reporting,
            verification,
        })
    }

    pub async fn report_playback(
        &self,
        authorization: &Transport,
        track: &PreparedTrack,
        report: PlaybackReport,
    ) -> Result<bool> {
        if report.kind == PlaybackReportKind::Loading
            && let Some(verification) = &track.verification
        {
            let _ = verification.initially_paused.set(report.paused);
        }
        let Some(reporting) = &track.reporting else {
            return Ok(false);
        };
        let registration = self.inner.playback_registration.lock().await;
        let mut reporting = reporting.lock().await;
        if *registration != reporting.registration || reporting.closed {
            return Ok(false);
        }
        let Some(selection) = track
            .dealer
            .selected
            .borrow()
            .as_ref()
            .filter(|selection| selection.file_id == track.file_id)
            .cloned()
        else {
            return Ok(false);
        };
        let headers = self.report_headers(authorization)?;
        let reference = reporting
            .updated_reference
            .as_ref()
            .filter(|(original, _)| original == &selection.reference)
            .map_or(&selection.reference, |(_, updated)| updated);
        let reference = reference.clone();
        let payload = reporting.payload(&reference, track.metadata.duration_ms, report)?;
        let response = tokio::time::timeout(
            REPORT_TIMEOUT,
            self.api_with_headers(
                Method::PUT,
                &format!("/track-playback/v1/devices/{}/state", self.inner.device_id),
                Some(payload),
                "playback report",
                headers,
            ),
        )
        .await
        .map_err(|_| Error::Timeout("playback report"))??;
        if let Some((selected, updated)) = selected_track(&response, "updated_state_ref") {
            if selected.pointer("/metadata/uri").and_then(Value::as_str)
                == Some(&track.metadata.uri)
                && audio_file_id(&selected["manifest"]["file_ids_mp4"][0]) == Some(&track.file_id)
            {
                reporting.updated_reference = Some((selection.reference, updated));
            } else {
                reporting.closed = true;
            }
        }
        Ok(true)
    }

    pub async fn report_stream_verification(
        &self,
        authorization: &Transport,
        track: &PreparedTrack,
        report: PlaybackReport,
    ) -> Result<bool> {
        const STAGE: &str = "stream verification";
        let Some(verification) = &track.verification else {
            return Ok(false);
        };
        if report.kind != PlaybackReportKind::Finalized
            || report.played_ms == 0
            || report.playback_speed != 1.0
        {
            return Ok(false);
        }
        let headers = self.report_headers(authorization)?;
        if verification.sent.swap(true, Ordering::Relaxed) {
            return Ok(false);
        }
        let sequence = verification.sequence(&self.inner.verification_sequence);
        let session = &self.inner.playback_session;
        let message = json!({
            "type": "track_stream_verification",
            "message": {
                "play_track": track.metadata.uri,
                "playback_id": verification.playback_id,
                "ms_played": report.played_ms,
                "ms_nominal_played": report.played_ms,
                "session_id": session,
                "sequence_id": sequence,
                "next_playback_id": "",
                "playback_service": "track-playback",
            },
        });
        self.send_melody_report(headers, stream_report_batch(vec![message]), STAGE)
            .await
    }

    pub async fn report_playback_stats(
        &self,
        authorization: &Transport,
        track: &PreparedTrack,
        report: PlaybackReport,
    ) -> Result<bool> {
        let Some(verification) = &track.verification else {
            return Ok(false);
        };
        let Some(timeline) = report.timeline.filter(PlaybackTimeline::is_ordered) else {
            return Ok(false);
        };
        if report.kind != PlaybackReportKind::Finalized
            || report.played_ms == 0
            || report.playback_speed != 1.0
        {
            return Ok(false);
        }
        let headers = self.report_headers(authorization)?;
        if verification.stats_sent.swap(true, Ordering::Relaxed) {
            return Ok(false);
        }
        let message = playback_stats_message(
            track,
            verification,
            report,
            timeline,
            &self.inner.playback_session,
            verification.sequence(&self.inner.verification_sequence),
        );
        self.send_melody_report(
            headers,
            stream_report_batch(vec![message]),
            "playback stats",
        )
        .await
    }

    pub async fn report_playback_start(
        &self,
        authorization: &Transport,
        track: &PreparedTrack,
        report: PlaybackReport,
    ) -> Result<bool> {
        let Some(verification) = &track.verification else {
            return Ok(false);
        };
        if report.kind != PlaybackReportKind::Started || report.played_ms == 0 {
            return Ok(false);
        }
        let headers = self.report_headers(authorization)?;
        if verification.start_sent.swap(true, Ordering::Relaxed) {
            return Ok(false);
        }
        let message =
            playback_start_message(track, verification, report, &self.inner.playback_session);
        self.send_melody_report(
            headers,
            stream_report_batch(vec![message]),
            "playback start",
        )
        .await
    }

    pub async fn report_media_playback(
        &self,
        authorization: &Transport,
        track: &PreparedTrack,
        report: PlaybackReport,
    ) -> Result<bool> {
        let Some(verification) = &track.verification else {
            return Ok(false);
        };
        let Some(timeline) = report.timeline.filter(PlaybackTimeline::is_ordered) else {
            return Ok(false);
        };
        if report.kind != PlaybackReportKind::Finalized || report.played_ms == 0 {
            return Ok(false);
        }
        let headers = self.report_headers(authorization)?;
        if verification.media_sent.swap(true, Ordering::Relaxed) {
            return Ok(false);
        }
        self.send_melody_report(
            headers,
            media_playback_payload(track, &verification.playback_id, report, timeline),
            "media playback",
        )
        .await
    }

    async fn send_melody_report(
        &self,
        headers: &HeaderMap,
        payload: Value,
        stage: &'static str,
    ) -> Result<bool> {
        let url = self
            .inner
            .spclient
            .join("/melody/v1/msg/batch")
            .map_err(|_| Error::Invalid("Melody endpoint"))?;
        let response = self
            .inner
            .http
            .post(url)
            .headers(headers.clone())
            .json(&payload)
            .timeout(REPORT_TIMEOUT)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    Error::Timeout(stage)
                } else {
                    Error::Network(stage)
                }
            })?;
        let status = response.status();
        if matches!(
            status,
            reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNPROCESSABLE_ENTITY
        ) {
            let diagnostic = body(response, REPORT_DIAGNOSTIC_LIMIT, stage)
                .await
                .map_or_else(
                    |_| "diagnostic unavailable".to_owned(),
                    |bytes| report_rejection(&bytes, &payload),
                );
            return Err(Error::ReportRejected {
                stage,
                status: status.as_u16(),
                diagnostic,
            });
        }
        if status != reqwest::StatusCode::ACCEPTED {
            return Err(Error::Http {
                stage,
                status: status.as_u16(),
            });
        }
        Ok(true)
    }

    fn report_headers<'a>(&self, authorization: &'a Transport) -> Result<&'a HeaderMap> {
        let authorization = &authorization.inner;
        if authorization.device_id != self.inner.device_id
            || authorization.license_cookie != self.inner.license_cookie
        {
            return Err(Error::AuthorizationDenied);
        }
        if authorization.expires_at <= now_ms()? {
            return Err(Error::AuthorizationExpired);
        }
        Ok(&authorization.headers)
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
        self.api_with_headers(method, path, payload, stage, &self.inner.headers)
            .await
    }

    async fn api_with_headers(
        &self,
        method: Method,
        path: &str,
        payload: Option<Value>,
        stage: &'static str,
        headers: &HeaderMap,
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
            .headers(headers.clone());
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
        let (selected_tx, selected) = watch::channel(None);
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
                        observe_dealer_payload(&value, &wanted, &manifest_tx, &selected_tx);
                        if let Some(payloads) = value["payloads"].as_array() {
                            for payload in payloads.iter().filter_map(Value::as_str) {
                                if let Some(payload) = decode_payload(payload) {
                                    observe_dealer_payload(&payload, &wanted, &manifest_tx, &selected_tx);
                                }
                            }
                        }
                    }
                }
            }
        });
        let dealer = Dealer { task, selected };
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

fn autoplay_track(value: &Value, seed: &str, recent: &[String]) -> Result<Option<CatalogTrack>> {
    let tracks = value["tracks"]
        .as_array()
        .ok_or(Error::Invalid("Spotify autoplay response"))?;
    Ok(tracks.iter().find_map(|track| {
        let uri = track["uri"].as_str()?;
        if uri == seed || recent.iter().any(|recent| recent == uri) || track_id(uri).is_err() {
            return None;
        }
        let metadata = &track["metadata"];
        let title = track["name"]
            .as_str()
            .or_else(|| metadata["title"].as_str())?;
        if title.trim().is_empty() {
            return None;
        }
        let cover_url = metadata["image_url"]
            .as_str()
            .and_then(|uri| uri.strip_prefix("spotify:image:"))
            .filter(|id| id.len() == 40 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .map(|id| format!("https://i.scdn.co/image/{id}"));
        let artists: Vec<_> = metadata["artist_name"]
            .as_str()
            .map(|name| ArtistCredit {
                name: name.to_owned(),
                uri: track["artist_uri"]
                    .as_str()
                    .and_then(crate::resource_uri)
                    .filter(|uri| uri.starts_with("spotify:artist:")),
            })
            .into_iter()
            .collect();
        Some(CatalogTrack {
            metadata: TrackMetadata {
                uri: uri.to_owned(),
                title: title.to_owned(),
                artist_names: artists.iter().map(|artist| artist.name.clone()).collect(),
                album: metadata["album_title"].as_str().map(str::to_owned),
                duration_ms: None,
                cover_url,
                explicit: metadata["is_explicit"].as_str() == Some("true"),
            },
            artists,
            album_uri: track["album_uri"]
                .as_str()
                .and_then(crate::resource_uri)
                .filter(|uri| uri.starts_with("spotify:album:")),
            saved: false,
            plays: None,
            playlist_item_uid: None,
            added_at: None,
        })
    }))
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

fn unsigned_number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

fn stream_report_batch(messages: Vec<Value>) -> Value {
    json!({
        "messages": messages,
        "sdk_id": HARMONY.split_once('-').map_or(HARMONY, |(version, _)| version),
        "platform": PLATFORM,
        "client_version": "0.0.0",
    })
}

fn report_rejection(bytes: &[u8], payload: &Value) -> String {
    let reply = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    let reason = if reply.contains("missing") || reply.contains("required") {
        "missing required data"
    } else if reply.contains("type") || reply.contains("expected") {
        "unexpected type or value"
    } else if reply.contains("unknown") || reply.contains("unsupported") {
        "unknown or unsupported data"
    } else if reply.contains("invalid") || reply.contains("malformed") {
        "invalid data"
    } else {
        "unclassified rejection"
    };
    let field = payload["messages"][0]["message"]
        .as_object()
        .and_then(|fields| {
            reply
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .find_map(|word| fields.get_key_value(word).map(|(field, _)| field))
        });
    field.map_or_else(
        || reason.to_owned(),
        |field| format!("{reason} for {field}"),
    )
}

fn playback_start_message(
    track: &PreparedTrack,
    verification: &StreamVerification,
    report: PlaybackReport,
    session: &str,
) -> Value {
    let mut message = json!({
        "play_track": track.metadata.uri,
        "file_id": track.file_id,
        "playback_id": verification.playback_id,
        "session_id": session,
        "ms_start_position": report.start_offset_ms,
        "feature_identifier": "",
    });
    if let Some(paused) = verification.initially_paused.get() {
        message["initially_paused"] = json!(paused);
    }
    json!({"type": "jssdk_playback_start", "message": message})
}

fn playback_stats_message(
    track: &PreparedTrack,
    verification: &StreamVerification,
    report: PlaybackReport,
    timeline: PlaybackTimeline,
    session: &str,
    sequence: u64,
) -> Value {
    const CODEC: &str = "mp4a.40.2";
    let media = verification.playback_media.get();
    let bitrate = verification.bitrate.filter(|bitrate| *bitrate > 0);
    let played_per_audio_format = bitrate.map_or_else(
        || json!({}),
        |bitrate| json!({ (format!("{CODEC};{bitrate}")): report.played_ms }),
    );
    let mut message = json!({
        "play_track": track.metadata.uri,
        "file_id": track.file_id,
        "manifest_id": track.file_id,
        "playback_id": verification.playback_id,
        "internal_play_id": verification.internal_play_id.to_string(),
        "session_id": session,
        "sequence_id": sequence,
        "audio_format": "mp4",
        "video_format": "",
        "protected": true,
        "key_system": "com.widevine.alpha",
        "key_system_impl": "native",
        "memory_cached": false,
        "persistent_cached": media.is_some_and(|media| media.persistent_cached),
        "urls_json": r#"{"version":"1.0.0","urls":[]}"#,
        "start_time": timeline.loaded_at_ms,
        "end_time": timeline.ended_at_ms,
        "external_start_time": 0,
        "ms_play_latency": timeline.started_at_ms - timeline.loaded_at_ms,
        "ms_played": report.played_ms,
        "ms_nominal_played": report.played_ms,
        "ms_file_duration": report.duration_ms,
        "ms_actual_duration": report.duration_ms,
        "ms_metadata_duration": track.metadata.duration_ms.unwrap_or_default(),
        "ms_start_position": report.start_offset_ms,
        "ms_end_position": report.position_ms,
        "ms_played_per_surface": {},
        "ms_played_per_audio_format": played_per_audio_format,
        "ms_played_per_video_format": {},
        "audio_start_bitrate": bitrate,
        "video_start_bitrate": null,
        "start_bitrate": bitrate.unwrap_or_default(),
        "reason_start": "remote",
        "reason_end": "trackdone",
        "initially_paused": verification.initially_paused.get().copied().unwrap_or_default(),
        "had_error": false,
        "client_id": "",
        "correlation_id": "",
        "total_bytes": media.map_or(0, |media| media.total_bytes),
        "strategy": "MSE",
    });
    for field in [
        "ms_pre_play_latency",
        "ms_init_latency",
        "ms_head_latency",
        "ms_first_bytes_latency",
        "ms_first_append_done_latency",
        "ms_key_system_info_latency",
        "ms_manifest_latency",
        "ms_manifest_parse_latency",
        "ms_resolve_latency",
        "ms_license_session_latency",
        "ms_license_generation_latency",
        "ms_license_request_latency",
        "ms_license_update_latency",
    ] {
        message[field] = Value::Null;
    }
    for field in [
        "ms_initial_rebuffer",
        "ms_seek_rebuffer",
        "ms_seek_rebuffer_longest",
        "ms_stall_rebuffer",
        "ms_stall_rebuffer_longest",
        "ms_played_visible",
        "n_stalls",
        "n_rendition_upgrade",
        "n_rendition_downgrade",
        "bps_bandwidth_max",
        "bps_bandwidth_min",
        "bps_bandwidth_avg",
        "n_seekback",
        "n_seekforward",
        "time_weighted_bitrate",
        "n_warnings",
        "n_navigator_offline",
        "n_dropped_video_frames",
        "n_total_video_frames",
        "resolution_max",
        "resolution_min",
    ] {
        message[field] = json!(0);
    }
    json!({"type": "jssdk_playback_stats", "message": message})
}

fn media_playback_payload(
    track: &PreparedTrack,
    playback_id: &str,
    report: PlaybackReport,
    timeline: PlaybackTimeline,
) -> Value {
    json!({
        "messages": [{
            "type": "media_playback_event_v2",
            "message": {
                "events": [{
                    "sn": 0,
                    "ct": timeline.loaded_at_ms,
                    "ms": {
                        "mu": format!("spotify-audio://{}", track.file_id),
                        "fi": "",
                        "pi": playback_id,
                        "si": "",
                        "mt": "audio",
                    },
                    "ps": {
                        "md": 0,
                        "ip": true,
                        "pp": report.start_offset_ms,
                        "sp": 1,
                        "br": 0,
                        "pd": "device",
                        "ao": true,
                    },
                    "et": 0,
                    "ur": track.metadata.uri,
                    "pc": "",
                    "xt": "audio",
                }, {
                    "sn": 1,
                    "ct": timeline.started_at_ms,
                    "ms": {},
                    "ps": {"pp": report.start_offset_ms, "md": report.duration_ms},
                    "et": 4,
                }, {
                    "sn": 2,
                    "ct": timeline.ended_at_ms,
                    "ms": {},
                    "ps": {"md": report.duration_ms, "pp": report.position_ms},
                    "et": 10,
                    "rn": 5,
                    "bt": -1,
                    "rf": 0,
                    "df": 0,
                }],
                "context": {
                    "device": {"manufacturer": "spotify", "model": "web_player", "type": "computer"},
                    "os": {"name": "", "version": ""},
                },
            },
        }],
        "sdk_id": HARMONY,
        "platform": MEDIA_REPORT_PLATFORM,
        "client_version": "0.0.0",
    })
}

fn audio_file_id(file: &Value) -> Option<&str> {
    file["file_id"]
        .as_str()
        .filter(|id| id.len() == 40 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn selected_track<'a>(value: &'a Value, reference: &str) -> Option<(&'a Value, PlaybackReference)> {
    let machine = &value["state_machine"];
    let state_index = usize::try_from(unsigned_number(&value[reference]["state_index"])?).ok()?;
    let state = machine["states"].get(state_index)?;
    let track_index = usize::try_from(unsigned_number(&state["track"])?).ok()?;
    let track = machine["tracks"].get(track_index)?;
    let reference = PlaybackReference {
        state_machine_id: machine["state_machine_id"].as_str()?.to_owned(),
        state_id: state["state_id"].as_str()?.to_owned(),
    };
    Some((track, reference))
}

fn find_replace_state(value: &Value, depth: usize) -> Option<&Value> {
    if depth > 14 {
        return None;
    }
    if value["type"].as_str() == Some("replace_state") {
        return Some(value);
    }
    match value {
        Value::Object(values) => values
            .values()
            .find_map(|value| find_replace_state(value, depth + 1)),
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_replace_state(value, depth + 1)),
        _ => None,
    }
}

fn observe_dealer_payload(
    value: &Value,
    uri: &str,
    manifests: &watch::Sender<Option<Value>>,
    selections: &watch::Sender<Option<SelectedPlayback>>,
) {
    if let Some(command) = find_replace_state(value, 0) {
        if let Some((track, reference)) = selected_track(command, "state_ref")
            && track.pointer("/metadata/uri").and_then(Value::as_str) == Some(uri)
            && let Some(file_id) = audio_file_id(&track["manifest"]["file_ids_mp4"][0])
        {
            let _ = selections.send(Some(SelectedPlayback {
                reference,
                file_id: file_id.to_owned(),
            }));
            let _ = manifests.send(Some(track.clone()));
            return;
        }
        let _ = selections.send(None);
    }
    if let Some(track) = find_manifest(value, uri, 0) {
        let _ = manifests.send(Some(track.clone()));
    }
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
                playback_registration: Mutex::new(0),
                playback_session: "123456789".to_owned(),
                verification_sequence: AtomicU64::new(0),
            }),
        }
    }

    #[test]
    fn autoplay_preserves_rank_after_skipping_recent_and_invalid_tracks() {
        let seed = "spotify:track:0000000000000000000001";
        let recent = "spotify:track:0000000000000000000002";
        let first = "spotify:track:0000000000000000000003";
        let second = "spotify:track:0000000000000000000004";
        let response = json!({"tracks": [
            {"uri": seed, "name": "Seed"},
            {"uri": recent, "name": "Recently played"},
            {"uri": "spotify:episode:0000000000000000000005", "name": "Episode"},
            {"uri": first, "artist_uri": "spotify:artist:0000000000000000000006",
                "album_uri": "spotify:album:0000000000000000000007", "metadata": {
                "title": "First", "artist_name": "Artist", "album_title": "Album",
                "image_url": "spotify:image:0123456789abcdef0123456789abcdef01234567",
                "is_explicit": "true"
            }},
            {"uri": second, "name": "Second"}
        ]});
        let track = autoplay_track(&response, seed, &[recent.to_owned()])
            .unwrap()
            .unwrap();
        assert_eq!(track.metadata.uri, first);
        assert_eq!(track.metadata.title, "First");
        assert_eq!(track.metadata.artist_names, ["Artist"]);
        assert_eq!(track.metadata.album.as_deref(), Some("Album"));
        assert!(track.artists[0].uri.is_some());
        assert!(track.album_uri.is_some());
        assert_eq!(
            track.metadata.cover_url.as_deref(),
            Some("https://i.scdn.co/image/0123456789abcdef0123456789abcdef01234567")
        );
        assert!(track.metadata.explicit);
        assert!(
            autoplay_track(&json!({"tracks": []}), seed, &[])
                .unwrap()
                .is_none()
        );
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
                selected: watch::channel(None).1,
            },
            reporting: None,
            verification: None,
        }
    }

    #[test]
    fn dealer_reports_only_the_selected_track_and_audio_file() {
        let requested = "spotify:track:0000000000000000000000";
        let file = "0123456789abcdef0123456789abcdef01234567";
        let mut message = json!({
            "payloads": [{
                "type": "replace_state",
                "state_ref": { "state_index": 0 },
                "state_machine": {
                    "state_machine_id": "machine",
                    "states": [
                        {"state_id": "other", "track": 0},
                        {"state_id": "requested", "track": 1},
                    ],
                    "tracks": [
                        {"metadata": {"uri": "spotify:track:other"}},
                        {
                            "metadata": {"uri": requested},
                            "manifest": {"file_ids_mp4": [{"file_id": file}]},
                        },
                    ],
                },
            }],
        });
        let (manifest_tx, manifests) = watch::channel(None);
        let (selection_tx, selections) = watch::channel(None);
        observe_dealer_payload(&message, requested, &manifest_tx, &selection_tx);
        assert!(manifests.borrow().is_some());
        assert!(selections.borrow().is_none());

        message["payloads"][0]["state_ref"]["state_index"] = json!(1);
        observe_dealer_payload(&message, requested, &manifest_tx, &selection_tx);
        let selections = selections.borrow();
        let selection = selections.as_ref().unwrap();
        assert_eq!(selection.file_id, file);
        assert_eq!(selection.reference.state_machine_id, "machine");
        assert_eq!(selection.reference.state_id, "requested");
    }

    #[test]
    fn playback_reports_preserve_actual_positions_and_sequence() {
        let mut reporting = ReportingState {
            registration: 1,
            sequence: 19,
            updated_reference: None,
            closed: false,
            bitrate: Some(128_000),
            format: Some(19),
            audio_quality: None,
        };
        let reference = PlaybackReference {
            state_machine_id: "machine".to_owned(),
            state_id: "track".to_owned(),
        };
        let threshold = PlaybackReport {
            kind: PlaybackReportKind::ThresholdReached,
            position_ms: 35_500,
            previous_position_ms: Some(35_250),
            duration_ms: 90_000,
            played_ms: 31_500,
            start_offset_ms: 4_000,
            paused: false,
            playback_speed: 1.0,
            timeline: None,
        };
        let first = reporting
            .payload(&reference, Some(90_100), threshold)
            .unwrap();
        assert_eq!(first["seq_num"], 20);
        assert_eq!(first["previous_position"], 35_500);
        assert_eq!(first["debug_source"], "played_threshold_reached");
        assert_eq!(first["sub_state"]["media_type"], "AUDIO");
        assert!(first.get("playback_stats").is_none());
        assert!(first["sub_state"].get("audio_quality").is_none());

        let finalized = PlaybackReport {
            kind: PlaybackReportKind::Finalized,
            position_ms: 90_000,
            previous_position_ms: Some(89_750),
            played_ms: 80_000,
            paused: true,
            ..threshold
        };
        let second = reporting
            .payload(&reference, Some(90_100), finalized)
            .unwrap();
        assert_eq!(second["seq_num"], 21);
        assert_eq!(second["debug_source"], "track_data_finalized");
        assert_eq!(second["sub_state"]["position"], 90_000);
        assert_eq!(second["sub_state"]["playback_speed"], 0.0);
        assert_eq!(second["previous_position"], 89_750);
        assert_eq!(second["playback_stats"]["ms_metadata_duration"], 90_100);
        assert_eq!(
            second["playback_stats"]["ms_played_per_audio_format"]["mp4a.40.2;128000"],
            80_000
        );

        let loading = PlaybackReport {
            kind: PlaybackReportKind::Loading,
            position_ms: 0,
            previous_position_ms: None,
            played_ms: 0,
            ..threshold
        };
        let third = reporting.payload(&reference, None, loading).unwrap();
        assert_eq!(third["sub_state"]["playback_speed"], 0.0);
        assert!(third.get("previous_position").is_none());
        let started = PlaybackReport {
            kind: PlaybackReportKind::Started,
            position_ms: 1_250,
            previous_position_ms: Some(1_000),
            played_ms: 1_250,
            ..threshold
        };
        let fourth = reporting.payload(&reference, None, started).unwrap();
        assert_eq!(fourth["sub_state"]["playback_speed"], 1.0);
        assert_eq!(fourth["previous_position"], 1_250);

        let paused = PlaybackReport {
            kind: PlaybackReportKind::Paused,
            paused: true,
            ..started
        };
        let pause_payload = reporting.payload(&reference, None, paused).unwrap();
        assert_eq!(pause_payload["debug_source"], "pause");
        assert_eq!(pause_payload["state_ref"]["paused"], true);
        assert_eq!(pause_payload["sub_state"]["playback_speed"], 0.0);
        assert_eq!(pause_payload["previous_position"], 1_250);

        let seeked = PlaybackReport {
            kind: PlaybackReportKind::Seeked,
            previous_position_ms: Some(1_250),
            position_ms: 22_000,
            ..paused
        };
        let seek_payload = reporting.payload(&reference, None, seeked).unwrap();
        assert_eq!(seek_payload["debug_source"], "seek");
        assert_eq!(seek_payload["sub_state"]["position"], 22_000);
        assert_eq!(seek_payload["previous_position"], 1_250);
        assert_eq!(seek_payload["sub_state"]["playback_speed"], 0.0);

        let resumed = PlaybackReport {
            kind: PlaybackReportKind::Resumed,
            paused: false,
            ..seeked
        };
        let resume_payload = reporting.payload(&reference, None, resumed).unwrap();
        assert_eq!(resume_payload["debug_source"], "resume");
        assert_eq!(resume_payload["state_ref"]["paused"], false);
        assert_eq!(resume_payload["sub_state"]["playback_speed"], 1.0);
        assert_eq!(resume_payload["previous_position"], 22_000);
    }

    #[tokio::test]
    async fn stream_verification_uses_refreshed_credentials_and_the_original_play() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut original = transport(0);
        Arc::get_mut(&mut original.inner).unwrap().spclient =
            Url::parse(&format!("http://{}", server.local_addr().unwrap())).unwrap();
        let mut authorization = transport(u64::MAX);
        let auth = Arc::get_mut(&mut authorization.inner).unwrap();
        auth.playback_session = "different-auth-session".to_owned();
        auth.headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer refreshed-test"),
        );
        let mut unrelated = transport(u64::MAX);
        Arc::get_mut(&mut unrelated.inner).unwrap().license_cookie =
            HeaderValue::from_static("sp_dc=unrelated");
        let mut track = prepared_track("file");
        let verification = StreamVerification::new("original-state".to_owned(), 5, Some(128_000));
        verification.initially_paused.set(true).unwrap();
        track.verification = Some(verification);
        track.record_playback_media(12_345, true);
        let report = PlaybackReport {
            kind: PlaybackReportKind::Finalized,
            position_ms: 90_000,
            previous_position_ms: Some(89_750),
            duration_ms: 90_000,
            played_ms: 80_000,
            start_offset_ms: 0,
            paused: true,
            playback_speed: 1.0,
            timeline: Some(PlaybackTimeline {
                loaded_at_ms: 100_000,
                started_at_ms: 100_800,
                ended_at_ms: 180_800,
            }),
        };
        assert!(
            !original
                .report_stream_verification(
                    &authorization,
                    &track,
                    PlaybackReport {
                        played_ms: 0,
                        ..report
                    }
                )
                .await
                .unwrap()
        );
        assert!(matches!(
            original
                .report_stream_verification(&original, &track, report)
                .await,
            Err(Error::AuthorizationExpired)
        ));
        assert!(matches!(
            original
                .report_stream_verification(&unrelated, &track, report)
                .await,
            Err(Error::AuthorizationDenied)
        ));
        let server = &server;
        let read_request = |status, response_body: &str| {
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            async move {
                let (socket, _) = server.accept().await.unwrap();
                let mut socket = BufReader::new(socket);
                let mut line = String::new();
                socket.read_line(&mut line).await.unwrap();
                assert_eq!(line, "POST /melody/v1/msg/batch HTTP/1.1\r\n");
                let mut length = None;
                let mut bearer = false;
                loop {
                    line.clear();
                    assert_ne!(socket.read_line(&mut line).await.unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = Some(value.trim().parse::<usize>().unwrap());
                        } else if name.eq_ignore_ascii_case("authorization") {
                            bearer = value.trim() == "Bearer refreshed-test";
                        }
                    }
                }
                assert!(bearer);
                let mut body = vec![0; length.unwrap()];
                socket.read_exact(&mut body).await.unwrap();
                socket.write_all(response.as_bytes()).await.unwrap();
                serde_json::from_slice::<Value>(&body).unwrap()
            }
        };
        let (receipt, payload) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(
                original.report_playback_stats(&authorization, &track, report),
                read_request(
                    "400 Bad Request",
                    r#"{"message":"missing required ms_actual_duration","token":"must-not-log"}"#,
                )
            )
        })
        .await
        .unwrap();
        let error = receipt.unwrap_err().to_string();
        assert!(error.contains("missing required data for ms_actual_duration"));
        assert!(!error.contains("must-not-log"));
        assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
        let stats = &payload["messages"][0];
        assert_eq!(stats["type"], "jssdk_playback_stats");
        assert_eq!(stats["message"].as_object().unwrap().len(), 74);
        assert_eq!(stats["message"]["playback_id"], "original-state");
        assert_eq!(stats["message"]["internal_play_id"], "5");
        assert_eq!(stats["message"]["session_id"], "123456789");
        assert_eq!(stats["message"]["sequence_id"], 0);
        assert_eq!(stats["message"]["start_time"], 100_000);
        assert_eq!(stats["message"]["end_time"], 180_800);
        assert_eq!(stats["message"]["ms_play_latency"], 800);
        assert_eq!(stats["message"]["reason_end"], "trackdone");
        assert_eq!(stats["message"]["initially_paused"], true);
        assert_eq!(stats["message"]["persistent_cached"], true);
        assert_eq!(stats["message"]["total_bytes"], 0);
        assert_eq!(stats["message"]["strategy"], "MSE");
        assert_eq!(stats["message"]["ms_license_update_latency"], Value::Null);
        assert_eq!(
            stats["message"]["ms_played_per_audio_format"]["mp4a.40.2;128000"],
            80_000
        );

        let (receipt, payload) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(
                original.report_stream_verification(&authorization, &track, report),
                read_request("202 Accepted", "")
            )
        })
        .await
        .unwrap();
        assert!(receipt.unwrap());
        let message = &payload["messages"][0]["message"];
        assert_eq!(payload["messages"].as_array().unwrap().len(), 1);
        assert_eq!(message["playback_id"], "original-state");
        assert_eq!(message["ms_played"], 80_000);
        assert_eq!(message["ms_nominal_played"], 80_000);
        assert_eq!(message["session_id"], "123456789");
        assert_eq!(message["sequence_id"], 0);
        let started = playback_start_message(
            &track,
            track.verification.as_ref().unwrap(),
            PlaybackReport {
                kind: PlaybackReportKind::Started,
                start_offset_ms: 1_250,
                ..report
            },
            "123456789",
        );
        assert_eq!(started["type"], "jssdk_playback_start");
        assert_eq!(started["message"]["playback_id"], "original-state");
        assert_eq!(started["message"]["ms_start_position"], 1_250);
        assert_eq!(started["message"]["initially_paused"], true);
        assert!(
            !original
                .report_stream_verification(&authorization, &track, report)
                .await
                .unwrap()
        );
        assert!(
            !original
                .report_playback_stats(&authorization, &track, report)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn media_playback_payload_reports_natural_end_from_measured_timeline() {
        let track = prepared_track("encrypted-file");
        let timeline = PlaybackTimeline {
            loaded_at_ms: 100_000,
            started_at_ms: 100_800,
            ended_at_ms: 190_800,
        };
        let report = PlaybackReport {
            kind: PlaybackReportKind::Finalized,
            position_ms: 92_000,
            previous_position_ms: Some(91_750),
            duration_ms: 92_000,
            played_ms: 90_000,
            start_offset_ms: 2_000,
            paused: false,
            playback_speed: 1.0,
            timeline: Some(timeline),
        };
        let body = media_playback_payload(&track, "initial-playback-state", report, timeline);
        let message = &body["messages"][0];
        assert_eq!(message["type"], "media_playback_event_v2");
        let events = message["message"]["events"].as_array().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["ct"], 100_000);
        assert_eq!(events[0]["ms"]["pi"], "initial-playback-state");
        assert_eq!(events[0]["ms"]["mu"], "spotify-audio://encrypted-file");
        assert_eq!(events[0]["ur"], track.metadata.uri);
        assert_eq!(events[1]["et"], 4);
        assert_eq!(events[1]["ct"], 100_800);
        assert_eq!(events[1]["ps"]["pp"], 2_000);
        assert_eq!(events[2]["ct"], 190_800);
        assert_eq!(events[2]["et"], 10);
        assert_eq!(events[2]["rn"], 5);
        assert_eq!(events[2]["ps"]["pp"], 92_000);
        assert_eq!(body["sdk_id"], HARMONY);
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
