//! Discord Rich Presence.
//!
//! Discord's IPC is a blocking Unix socket (a named pipe on Windows), so — exactly like the tray
//! icon on Linux — it gets a thread of its own with a channel in front of it. The app hands over a
//! plain [`Presence`] snapshot and never touches the crate's client; the worker owns connecting,
//! reconnecting when Discord is started late (or restarted), and withdrawing a presence that has
//! been paused too long.
//!
//! The app decides *what* to show and *when* it changed; the worker decides *how* to say it. That
//! split keeps the socket's blocking calls and the crate's own types out of the UI thread, and
//! makes the presence's shape (and its tests) independent of the transport.
//!
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use discord_rich_presence::activity::{
    Activity, ActivityType, Assets, Button, StatusDisplayType, Timestamps,
};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

use crate::model::Provider;

/// The worker's tick: fast enough that a toggle or a quit is noticed promptly, slow enough to
/// cost nothing while idle.
const TICK: Duration = Duration::from_millis(250);
/// How long between attempts to reach Discord while it is not running. Discord may simply not be
/// open yet, so this is a retry, not an error.
const RETRY_EVERY: Duration = Duration::from_secs(10);
/// How long a pause may stand before the presence is withdrawn: a paused client left open all
/// day should stop advertising itself.
const PAUSE_GRACE: Duration = Duration::from_secs(30);
/// Discord's own ceiling on the two text fields. A longer string is rejected outright.
const TEXT_LIMIT: usize = 128;
/// Discord only accepts a small image or a cover *URL* when it is short; some signed
/// `googleusercontent.com` URLs run past this.
const URL_LIMIT: usize = 256;
/// The cover is requested at this size — large enough to stay sharp wherever Discord draws it,
/// and the same length in the URL whatever number is chosen.
const COVER_PX: u32 = 544;

/// Moosik's public Discord application ID, shared by all desktop builds.
const APPLICATION_ID: &str = "1568322186417274880";

/// What the presence should say, assembled by the app.
///
/// A snapshot, not a handle: `PartialEq` is what lets the app skip a hand-off when nothing
/// material changed, and there is nothing Discord-specific in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Presence {
    pub provider: Provider,
    pub spotify_id: Option<String>,
    /// The video id, for the watch button. `None` for a local file, which has no page to open.
    pub youtube_id: Option<String>,
    pub room: Option<String>,
    pub listeners: usize,
    pub title: String,
    /// The credited artists, already joined.
    pub artist: String,
    /// The cover URL the model carries, used as a fallback when there is no video id.
    pub thumbnail: Option<String>,
    /// A music video rather than an audio track. The model's `wide` flag: it decides which still
    /// stands in when there is no square cover.
    pub wide: bool,
    pub playing: bool,
    /// Start and end of a playing track with known duration, in Unix seconds.
    pub timeline: Option<(i64, i64)>,
}

impl Presence {
    /// The activity Discord is sent. Public only through [`Discord::set`].
    fn activity(&self) -> Activity<'_> {
        let source = self.provider.name();
        let activity_name = format!("{source} via Moosik.dev");
        let mut activity = Activity::new()
            .activity_type(ActivityType::Listening)
            .name(activity_name)
            .status_display_type(StatusDisplayType::Details);
        // Discord also uses details for the compact member-list status.
        if let Some(title) = field(&self.title) {
            activity = activity.details(title);
            if self.listeners > 1 {
                let suffix = format!("{} listening", self.listeners);
                let artist: String = self
                    .artist
                    .trim()
                    .chars()
                    .take(TEXT_LIMIT - suffix.len() - 3)
                    .collect();
                activity = activity.state(if artist.is_empty() {
                    suffix
                } else {
                    format!("{artist} · {suffix}")
                });
            } else if let Some(artist) = field(&self.artist) {
                activity = activity.state(artist);
            }
        } else if let Some(artist) = field(&self.artist) {
            activity = activity.details(artist);
        }
        if self.playing
            && let Some((start, end)) = self.timeline
        {
            activity = activity.timestamps(
                Timestamps::new()
                    .start(start.saturating_mul(1000))
                    .end(end.saturating_mul(1000)),
            );
        }
        let youtube_id = (self.provider == Provider::YouTubeMusic)
            .then_some(self.youtube_id.as_deref())
            .flatten();
        let badge = if self.playing {
            "https://moosik.dev/assets/discord/playing.gif"
        } else {
            "https://moosik.dev/assets/discord/paused.png"
        };
        let mut assets = Assets::new().small_image(badge).small_text("Moosik.dev");
        if let Some(image) = cover_url(youtube_id, self.thumbnail.as_deref(), self.wide) {
            assets = assets.large_image(image);
        }
        activity = activity.assets(assets);
        let mut buttons = Vec::new();
        if let Some(id) = youtube_id
            && moosik_resolve::VideoId::new(id).is_ok()
        {
            buttons.push(Button::new(
                "Play on Moosik",
                format!("https://moosik.dev/#play/{id}"),
            ));
        }
        if self.provider == Provider::Spotify
            && let Some(id) = self
                .spotify_id
                .as_deref()
                .filter(|id| crate::model::is_spotify_id(id))
        {
            buttons.push(Button::new("Play on Spotify", crate::model::track_url(id)));
        }
        if let Some(room) = &self.room {
            buttons.push(Button::new(
                "Listen along",
                format!("https://moosik.dev/#join/{room}"),
            ));
        }
        if buttons.is_empty() {
            activity
        } else {
            activity.buttons(buttons)
        }
    }
}

/// A text field, or `None` when it is empty (an empty line is not worth sending) — truncated to
/// what Discord accepts, on a character boundary.
fn field(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(TEXT_LIMIT).collect())
}

/// The cover to send: the model's own, square cover art when it carries a crop suffix and fits
/// Discord's URL limit, else the short `i.ytimg` still, else nothing.
///
/// The model's cover is the album art itself, so it is preferred. The `i.ytimg` stills are
/// generated from the upload; for an art track that upload is the square art letterboxed into a
/// wider frame, and Discord draws the large image as a square by cropping the *centre*, which
/// keeps those baked-in bars. So the fallback still is picked by kind: a music video's own 16:9
/// `hq720` frame (the centre crop of which is all picture), and `hqdefault` only for a track that
/// has no square art anywhere.
fn cover_url(youtube_id: Option<&str>, thumbnail: Option<&str>, wide: bool) -> Option<String> {
    // The `i.ytimg.com` stills never make it through here: they are the generated, possibly
    // letterboxed frames, and the fallback below knows to ask for the right one by kind. Every
    // other host is real album art.
    if let Some(thumbnail) = thumbnail
        && !thumbnail.contains("i.ytimg.com")
        && url::Url::parse(thumbnail)
            .is_ok_and(|url| matches!(url.scheme(), "https" | "http") && url.host_str().is_some())
    {
        // Rewritten square, both so Discord crops nothing and so the request is for exactly the
        // pixels it draws — the model's cover can arrive as any size suffix.
        let sized = crate::images::sized_url(thumbnail, COVER_PX);
        if sized.len() <= URL_LIMIT {
            return Some(sized);
        }
    }
    let id = youtube_id.filter(|id| moosik_resolve::VideoId::new(*id).is_ok())?;
    let still = if wide { "hq720" } else { "hqdefault" };
    Some(format!("https://i.ytimg.com/vi/{id}/{still}.jpg"))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) struct SessionProfile {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
}

fn valid_avatar_hash(hash: &str) -> bool {
    let hash = hash.strip_prefix("a_").unwrap_or(hash);
    hash.len() == 32
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A participant may only select a Discord CDN avatar, never an arbitrary image URL.
pub(crate) fn avatar_url(key: &str) -> Option<Arc<str>> {
    let path = if let Some(index) = key.strip_prefix("default/") {
        if !matches!(index, "0" | "1" | "2" | "3" | "4" | "5") {
            return None;
        }
        format!("embed/avatars/{index}")
    } else {
        let (id, hash) = key.split_once('/')?;
        if id.is_empty()
            || id.len() > 20
            || !id.bytes().all(|b| b.is_ascii_digit())
            || !valid_avatar_hash(hash)
        {
            return None;
        }
        format!("avatars/{id}/{hash}")
    };
    Some(format!("https://cdn.discordapp.com/{path}.png?size=64").into())
}

impl SessionProfile {
    fn from_ready(user: &serde_json::Value) -> Self {
        let name = user["global_name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| user["username"].as_str())
            .unwrap_or_default()
            .chars()
            .take(32)
            .collect();
        let avatar = user["id"]
            .as_str()
            .and_then(|id| id.parse::<u64>().ok())
            .map(|id| {
                if let Some(hash) = user["avatar"].as_str().filter(|s| valid_avatar_hash(s)) {
                    format!("{id}/{hash}")
                } else {
                    let discriminator = user["discriminator"]
                        .as_str()
                        .and_then(|s| s.parse::<u16>().ok())
                        .unwrap_or(0);
                    let index = if discriminator == 0 {
                        (id >> 22) % 6
                    } else {
                        u64::from(discriminator % 5)
                    };
                    format!("default/{index}")
                }
            });
        Self { name, avatar }
    }
}

/// A live integration: the channel to its worker thread. Dropping this stops the worker (and
/// clears the presence), so the handle is held for as long as the setting is on.
pub struct Discord {
    connected: Arc<AtomicBool>,
    tx: Sender<Cmd>,
    profile: Arc<Mutex<SessionProfile>>,
}

impl Discord {
    /// Start the worker. Nothing connects until the first [`set`](Self::set), and the worker keeps
    /// trying in the background if Discord is not running yet.
    pub fn spawn(ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let connected = Arc::new(AtomicBool::new(false));
        let profile = Arc::new(Mutex::new(SessionProfile::default()));
        let worker_profile = profile.clone();
        let worker_connected = Arc::clone(&connected);
        let _ = std::thread::Builder::new()
            .name("discord-rpc".into())
            .spawn(move || run(rx, worker_connected, worker_profile, ctx));
        Self {
            tx,
            connected,
            profile,
        }
    }

    pub(crate) fn session_profile(&self) -> SessionProfile {
        self.profile.lock().unwrap().clone()
    }

    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Publish a presence. Cheap: a send on a channel.
    pub fn set(&self, presence: Presence) {
        let _ = self.tx.send(Cmd::Set(Box::new(presence)));
    }

    /// Withdraw the presence without stopping the worker.
    pub fn clear(&self) {
        let _ = self.tx.send(Cmd::Clear);
    }
}

impl Drop for Discord {
    fn drop(&mut self) {
        // Not joined: the worker's handshake `recv` has no timeout (the crate exposes none), and
        // an unresponsive Discord must not be able to hold up the app's own exit. The socket
        // closing with the process clears the presence anyway; this just asks nicely first.
        let _ = self.tx.send(Cmd::Shutdown);
    }
}

enum Cmd {
    Set(Box<Presence>),
    Clear,
    Shutdown,
}

#[derive(Default)]
struct WantedPresence {
    track: Option<Presence>,
    // Keep the deadline even after withdrawal: a seek or a room update is still the same pause.
    pause_deadline: Option<Instant>,
}

impl WantedPresence {
    fn set(&mut self, track: Presence, now: Instant) {
        if track.playing {
            self.pause_deadline = None;
        } else {
            self.pause_deadline.get_or_insert(now + PAUSE_GRACE);
        }
        self.track = Some(track);
    }

    fn expire_pause(&mut self, now: Instant) -> bool {
        self.pause_deadline.is_some_and(|deadline| now >= deadline) && self.track.take().is_some()
    }
}

/// The worker's whole life: own the socket, keep it in sync with the last `Presence`, and get it
/// back if Discord goes away.
fn run(
    rx: mpsc::Receiver<Cmd>,
    connected: Arc<AtomicBool>,
    profile: Arc<Mutex<SessionProfile>>,
    ctx: egui::Context,
) {
    let mut client: Option<DiscordIpcClient> = None;
    // What the app last asked for, and whether Discord has been told about it.
    let mut wanted = WantedPresence::default();
    let mut dirty = false;
    let mut next_try = Instant::now();

    loop {
        match rx.recv_timeout(TICK) {
            Ok(Cmd::Shutdown) => {
                if let Some(client) = client.as_mut() {
                    let _ = client.clear_activity();
                    let _ = client.close();
                }
                return;
            }
            Ok(Cmd::Clear) => {
                wanted = WantedPresence::default();
                dirty = true;
            }
            Ok(Cmd::Set(presence)) => {
                wanted.set(*presence, Instant::now());
                dirty = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        // A pause that has outlasted the grace period is withdrawn — but the worker keeps
        // running, so resuming shows up again immediately.
        if wanted.expire_pause(Instant::now()) {
            dirty = true;
        }

        // Connect (or reconnect) whenever there is something to show and it is time to try.
        if wanted.track.is_some() && client.is_none() && Instant::now() >= next_try {
            client = connect(&profile);
            next_try = Instant::now() + RETRY_EVERY;
            if client.is_some() {
                // Whatever happened while we were away has to be re-said.
                dirty = true;
            }
        }

        if dirty {
            // The end timestamp is absolute, so replaying a presence after a reconnect is still
            // correct: it does not drift with the wall clock the way a stored "remaining" would.
            let sent = client.as_mut().map(|client| match wanted.track.as_ref() {
                Some(presence) => client.set_activity(presence.activity()),
                None => client.clear_activity(),
            });
            match sent {
                Some(Ok(())) => {
                    if !connected.swap(true, Ordering::Relaxed) {
                        ctx.request_repaint();
                    }
                    dirty = false;
                }
                Some(Err(e)) => {
                    tracing::debug!(error = %e, "discord went away; will retry");
                    if connected.swap(false, Ordering::Relaxed) {
                        ctx.request_repaint();
                    }
                    *profile.lock().unwrap() = SessionProfile::default();
                    client = None;
                    next_try = Instant::now() + RETRY_EVERY;
                }
                // Not connected yet; the retry above will pick it up.
                None => {}
            }
        }
    }
}

/// One connection attempt. A failure is expected whenever Discord is not running, so it is logged
/// at debug and retried rather than surfaced.
fn connect(profile: &Mutex<SessionProfile>) -> Option<DiscordIpcClient> {
    let mut client = DiscordIpcClient::new(APPLICATION_ID);
    // The crate's default handshake discards READY, which carries the local display name.
    let ready = client
        .connect_ipc()
        .and_then(|_| {
            client.send(
                serde_json::json!({
                    "v": 1, "client_id": APPLICATION_ID
                }),
                0,
            )
        })
        .and_then(|_| client.recv());
    match ready {
        Ok((1, event)) if event["evt"] == "READY" => {
            *profile.lock().unwrap() = SessionProfile::from_ready(&event["data"]["user"]);
            tracing::debug!("connected to Discord");
            Some(client)
        }
        Ok(_) => None,
        Err(e) => {
            tracing::debug!(error = %e, "Discord is not reachable yet");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_pictures_use_discord_avatar_keys_only() {
        let hash = "a_0123456789abcdef0123456789abcdef";
        let user = serde_json::json!({ "id": "4194304", "global_name": "Sky", "avatar": hash });
        let profile = SessionProfile::from_ready(&user);
        assert_eq!(profile.name, "Sky");
        assert_eq!(
            avatar_url(profile.avatar.as_deref().unwrap()).as_deref(),
            Some(
                "https://cdn.discordapp.com/avatars/4194304/a_0123456789abcdef0123456789abcdef.png?size=64"
            )
        );
        for (discriminator, expected) in [("0", "default/1"), ("0009", "default/4")] {
            let user = serde_json::json!({ "id": "4194304", "discriminator": discriminator, "avatar": null });
            assert_eq!(
                SessionProfile::from_ready(&user).avatar.as_deref(),
                Some(expected)
            );
        }
        for invalid in [
            "https://example.com/image",
            "default/6",
            "default/1?redirect=x",
            "../0",
            "12/abcd",
        ] {
            assert!(avatar_url(invalid).is_none());
        }
    }

    fn presence() -> Presence {
        Presence {
            provider: Provider::YouTubeMusic,
            spotify_id: None,
            youtube_id: Some("dQw4w9WgXcQ".into()),
            room: None,
            listeners: 1,
            title: "Never Gonna Give You Up".into(),
            artist: "Rick Astley".into(),
            thumbnail: Some("https://lh3.googleusercontent.com/abc=w120-h120-l90-rj".into()),
            wide: false,
            playing: true,
            timeline: Some((1_699_999_800, 1_700_000_000)),
        }
    }

    /// Check the serialized payload sent to Discord through its write-only builder.
    #[test]
    fn a_playing_track_becomes_the_expected_activity() {
        let json = serde_json::to_value(presence().activity()).unwrap();
        assert_eq!(json["type"], 2, "Listening");
        assert_eq!(json["name"], "YouTube Music via Moosik.dev");
        assert_eq!(json["details"], "Never Gonna Give You Up");
        assert_eq!(json["state"], "Rick Astley");
        assert_eq!(json["status_display_type"], 2, "show the song title");
        assert_eq!(json["timestamps"]["start"], 1_699_999_800_000i64);
        assert_eq!(json["timestamps"]["end"], 1_700_000_000_000i64);
        // The model's square cover, rewritten to the size Discord draws — not the `i.ytimg`
        // still, whose art-track letterboxing survives Discord's centre crop as bars.
        assert_eq!(
            json["assets"]["large_image"],
            "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj"
        );
        assert_eq!(
            json["assets"]["small_image"],
            "https://moosik.dev/assets/discord/playing.gif"
        );
        assert!(json["assets"].get("large_text").is_none());
        assert_eq!(
            json["buttons"],
            serde_json::json!([
                {"label": "Play on Moosik", "url": "https://moosik.dev/#play/dQw4w9WgXcQ"}
            ])
        );
    }

    #[test]
    fn spotify_presence_uses_its_provider_and_public_song_link() {
        let spotify = Presence {
            provider: Provider::Spotify,
            spotify_id: Some("spotify:track:4uLU6hMCjMI75M1A2tKUQC".into()),
            thumbnail: Some("https://i.scdn.co/image/cover".into()),
            ..presence()
        };
        let json = serde_json::to_value(spotify.activity()).unwrap();
        assert_eq!(json["name"], "Spotify via Moosik.dev");
        assert_eq!(json["state"], "Rick Astley");
        assert_eq!(json["timestamps"]["start"], 1_699_999_800_000i64);
        assert_eq!(
            json["buttons"][0],
            serde_json::json!({
                "label": "Play on Spotify",
                "url": "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC"
            })
        );
        assert!(
            !serde_json::to_string(&json)
                .unwrap()
                .contains("dQw4w9WgXcQ")
        );
    }

    #[test]
    fn every_provider_keeps_title_and_artist_in_separate_fields() {
        for provider in Provider::ALL {
            let single = Presence {
                provider,
                ..presence()
            };
            let json = serde_json::to_value(single.activity()).unwrap();
            assert_eq!(json["details"], "Never Gonna Give You Up");
            assert_eq!(json["state"], "Rick Astley");
        }
    }

    #[test]
    fn local_presence_excludes_file_paths_and_remote_song_buttons() {
        let local = Presence {
            provider: Provider::Local,
            spotify_id: Some("spotify:track:4uLU6hMCjMI75M1A2tKUQC".into()),
            thumbnail: Some("file:///C:/Music/private/cover.jpg".into()),
            ..presence()
        };
        let json = serde_json::to_value(local.activity()).unwrap();
        assert_eq!(json["name"], "Local via Moosik.dev");
        assert_eq!(json["state"], "Rick Astley");
        assert!(json["assets"].get("large_image").is_none());
        assert!(json.get("buttons").is_none());
        let serialized = serde_json::to_string(&json).unwrap();
        assert!(!serialized.contains("C:/Music"));
        assert!(!serialized.contains("dQw4w9WgXcQ"));
        assert!(!serialized.contains("open.spotify.com"));
    }

    #[test]
    fn a_host_shares_the_invite_and_group_size() {
        let room = "a".repeat(32);
        let host = Presence {
            room: Some(room.clone()),
            listeners: 3,
            ..presence()
        };
        let json = serde_json::to_value(host.activity()).unwrap();
        assert_eq!(json["state"], "Rick Astley · 3 listening");
        assert_eq!(json["buttons"].as_array().unwrap().len(), 2);
        assert_eq!(
            json["buttons"][1],
            serde_json::json!({
                "label": "Listen along", "url": format!("https://moosik.dev/#join/{room}")
            })
        );
    }

    #[test]
    fn a_paused_hosts_card_expires_until_playback_resumes() {
        let now = Instant::now();
        let host = Presence {
            room: Some("room".into()),
            ..presence()
        };
        let paused = Presence {
            playing: false,
            timeline: None,
            ..host.clone()
        };
        let mut wanted = WantedPresence::default();
        wanted.set(paused.clone(), now);
        wanted.set(paused.clone(), now + PAUSE_GRACE / 2);
        assert!(!wanted.expire_pause(now + PAUSE_GRACE - TICK));
        assert_eq!(wanted.track.as_ref(), Some(&paused));
        assert!(wanted.expire_pause(now + PAUSE_GRACE));
        assert!(wanted.track.is_none());

        let later = now + Duration::from_secs(3600);
        wanted.set(paused, later);
        assert!(wanted.expire_pause(later));
        assert!(wanted.track.is_none());
        wanted.set(host.clone(), later);
        assert!(!wanted.expire_pause(later));
        assert_eq!(wanted.track, Some(host));
    }

    /// Paused means no timestamps at all — the bar must not keep counting while the track is
    /// stopped. Everything else stays, so the song is still shown.
    #[test]
    fn a_paused_track_carries_no_progress_bar() {
        let paused = Presence {
            playing: false,
            ..presence()
        };
        let json = serde_json::to_value(paused.activity()).unwrap();
        assert!(json.get("timestamps").is_none());
        assert_eq!(
            json["assets"]["small_image"],
            "https://moosik.dev/assets/discord/paused.png"
        );
        assert_eq!(json["details"], "Never Gonna Give You Up");
    }

    /// When the model carries no cover, or one too long for Discord's proxy, the short still
    /// stands in: something square beats no image, even though an art track's still is
    /// letterboxed.
    #[test]
    fn the_short_still_stands_in_when_the_model_cover_is_missing_or_too_long() {
        let no_cover = Presence {
            thumbnail: None,
            ..presence()
        };
        let json = serde_json::to_value(no_cover.activity()).unwrap();
        assert_eq!(
            json["assets"]["large_image"],
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
        );

        let too_long = Presence {
            thumbnail: Some("x".repeat(URL_LIMIT + 1)),
            ..presence()
        };
        let json = serde_json::to_value(too_long.activity()).unwrap();
        assert_eq!(
            json["assets"]["large_image"],
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
        );
    }

    /// A music video's row carries an `i.ytimg` still, signed with a query string. It is never
    /// rewritten as if that `=` were a crop, and since the still's 4:3 frame bakes letterbox bars
    /// into the picture, the 16:9 `hq720` frame stands in — Discord's centre crop of it is all
    /// picture.
    #[test]
    fn a_music_video_gets_its_wide_frame_not_the_letterboxed_still() {
        let video = Presence {
            thumbnail: Some(
                "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg?sqp=-oaymwEWCJAD&rs=AMzJL3k"
                    .into(),
            ),
            wide: true,
            ..presence()
        };
        let json = serde_json::to_value(video.activity()).unwrap();
        assert_eq!(
            json["assets"]["large_image"],
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/hq720.jpg"
        );

        // An audio track keeps the plain still when it has no cover of its own — square is worth
        // the fallback's letterboxing there.
        let song = Presence {
            thumbnail: Some(
                "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg?sqp=-oaymwEWCJAD&rs=AMzJL3k"
                    .into(),
            ),
            ..presence()
        };
        let json = serde_json::to_value(song.activity()).unwrap();
        assert_eq!(
            json["assets"]["large_image"],
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
        );
    }

    /// A local file has no YouTube page: no watch button, and no short thumbnail to fall back on
    /// unless the model happens to carry one that fits.
    #[test]
    fn a_local_track_has_no_watch_button_and_falls_back_to_its_own_cover() {
        let local = Presence {
            provider: Provider::Local,
            youtube_id: None,
            thumbnail: Some("https://example.test/cover.jpg".into()),
            ..presence()
        };
        let json = serde_json::to_value(local.activity()).unwrap();
        assert!(json.get("buttons").is_none());
        assert_eq!(
            json["assets"]["large_image"],
            "https://example.test/cover.jpg"
        );

        // A cover URL that is too long for Discord is dropped rather than sent to be rejected.
        let long = Presence {
            provider: Provider::Local,
            youtube_id: None,
            thumbnail: Some("x".repeat(URL_LIMIT + 1)),
            ..presence()
        };
        let json = serde_json::to_value(long.activity()).unwrap();
        assert!(json["assets"].get("large_image").is_none());
    }

    #[test]
    fn a_blank_title_shows_the_artist_once() {
        for title in ["", "   "] {
            let track = Presence {
                title: title.into(),
                ..presence()
            };
            let json = serde_json::to_value(track.activity()).unwrap();
            assert_eq!(json["details"], "Rick Astley");
            assert!(json.get("state").is_none());
        }
    }

    /// The two text fields are capped, and an empty one is left out entirely rather than sent as
    /// a blank line.
    #[test]
    fn text_fields_are_truncated_and_empties_dropped() {
        let long = Presence {
            title: "t".repeat(TEXT_LIMIT + 40),
            artist: "   ".into(),
            ..presence()
        };
        let json = serde_json::to_value(long.activity()).unwrap();
        assert_eq!(
            json["details"].as_str().unwrap().chars().count(),
            TEXT_LIMIT
        );
        assert!(json.get("state").is_none());
    }
}
