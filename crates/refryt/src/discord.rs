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
//! An id is required before any of this can connect, and [`DEFAULT_CLIENT_ID`] is the one every
//! install uses, so Rich Presence needs no setup from the user. It belongs to refryt's own Discord
//! application — an id cannot be created from code — and it is fixed: nothing in Settings changes
//! it, so every install shows up under the project's application and nowhere else.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use discord_rich_presence::activity::{
    Activity, ActivityType, Assets, Button, StatusDisplayType, Timestamps,
};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

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

/// The application every install connects as, so Rich Presence works with no setup. It belongs to
/// refryt's own Discord application; an id cannot be created from code, so this is the one
/// registered in the Developer Portal for the project. Hardcoded on purpose: one application id
/// for every install, nothing for a user to get wrong or to misattribute a presence through.
pub const DEFAULT_CLIENT_ID: &str = "1550438057642168411";

/// What the presence should say, assembled by the app.
///
/// A snapshot, not a handle: `PartialEq` is what lets the app skip a hand-off when nothing
/// material changed, and there is nothing Discord-specific in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Presence {
    /// The video id, for the watch button. `None` for a local file, which has no page to open.
    pub youtube_id: Option<String>,
    pub title: String,
    /// The credited artists, already joined. Empty hides the line.
    pub artist: String,
    pub album: Option<String>,
    /// The cover URL the model carries, used as a fallback when there is no video id.
    pub thumbnail: Option<String>,
    /// A music video rather than an audio track. The model's `wide` flag: it decides which still
    /// stands in when there is no square cover.
    pub wide: bool,
    pub playing: bool,
    /// When the track ends, in Unix seconds. Only set while playing; the bar is drawn from it.
    pub end_unix: Option<i64>,
}

impl Presence {
    /// The activity Discord is sent. Public only through [`Discord::set`].
    fn activity(&self) -> Activity<'_> {
        let mut activity = Activity::new()
            .activity_type(ActivityType::Listening)
            // Overrides the Discord application's own name, so the card reads "Listening to
            // YouTube Music via Moosik" instead of "Listening to refryt". The app is still the
            // one doing the listening; `refryt` is just not much of a name to broadcast.
            .name("YouTube Music via Moosik")
            // And the member list's one-liner is the artist, while the card itself still leads
            // with the song's title.
            .status_display_type(StatusDisplayType::State);
        if let Some(details) = field(&self.title) {
            activity = activity.details(details);
        }
        if let Some(state) = field(&self.artist) {
            activity = activity.state(state);
        }
        // Only while playing: a paused track has no end, and a bar that kept running would lie.
        if let Some(end) = self.end_unix {
            activity = activity.timestamps(Timestamps::new().end(end.saturating_mul(1000)));
        }
        if let Some(image) = cover_url(
            self.youtube_id.as_deref(),
            self.thumbnail.as_deref(),
            self.wide,
        ) {
            let mut assets = Assets::new().large_image(image);
            if let Some(text) = field(&self.title) {
                assets = assets.large_text(text);
            }
            activity = activity.assets(assets);
        }
        if let Some(id) = self.youtube_id.as_deref() {
            activity = activity.buttons(vec![Button::new("Play on YouTube Music", watch_url(id))]);
        }
        activity
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
    {
        // Rewritten square, both so Discord crops nothing and so the request is for exactly the
        // pixels it draws — the model's cover can arrive as any size suffix.
        let sized = crate::images::sized_url(thumbnail, COVER_PX);
        if sized.len() <= URL_LIMIT {
            return Some(sized);
        }
    }
    let id = youtube_id?;
    let still = if wide { "hq720" } else { "hqdefault" };
    Some(format!("https://i.ytimg.com/vi/{id}/{still}.jpg"))
}

fn watch_url(video_id: &str) -> String {
    format!("https://music.youtube.com/watch?v={video_id}")
}

/// A live integration: the channel to its worker thread. Dropping this stops the worker (and
/// clears the presence), so the handle is held for as long as the setting is on.
pub struct Discord {
    tx: Sender<Cmd>,
}

impl Discord {
    /// Start the worker. Nothing connects until the first [`set`](Self::set), and the worker keeps
    /// trying in the background if Discord is not running yet.
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("discord-rpc".into())
            .spawn(move || run(rx));
        Self { tx }
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

/// The worker's whole life: own the socket, keep it in sync with the last `Presence`, and get it
/// back if Discord goes away.
fn run(rx: mpsc::Receiver<Cmd>) {
    let mut client: Option<DiscordIpcClient> = None;
    // What the app last asked for, and whether Discord has been told about it.
    let mut wanted: Option<Presence> = None;
    let mut dirty = false;
    // When the current pause began, so it can be withdrawn after `PAUSE_GRACE`.
    let mut paused_since: Option<Instant> = None;
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
                wanted = None;
                paused_since = None;
                dirty = true;
            }
            Ok(Cmd::Set(presence)) => {
                if presence.playing {
                    paused_since = None;
                } else {
                    // Armed once per pause, not once per update: a seek while paused must not
                    // restart the grace period.
                    paused_since.get_or_insert_with(Instant::now);
                }
                wanted = Some(*presence);
                dirty = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        // A pause that has outlasted the grace period is withdrawn — but the worker keeps
        // running, so resuming shows up again immediately.
        if let Some(since) = paused_since
            && since.elapsed() >= PAUSE_GRACE
        {
            paused_since = None;
            wanted = None;
            dirty = true;
        }

        // Connect (or reconnect) whenever there is something to show and it is time to try.
        if wanted.is_some() && client.is_none() && Instant::now() >= next_try {
            client = connect();
            next_try = Instant::now() + RETRY_EVERY;
            if client.is_some() {
                // Whatever happened while we were away has to be re-said.
                dirty = true;
            }
        }

        if dirty {
            // The end timestamp is absolute, so replaying a presence after a reconnect is still
            // correct: it does not drift with the wall clock the way a stored "remaining" would.
            let sent = client.as_mut().map(|client| match wanted.as_ref() {
                Some(presence) => client.set_activity(presence.activity()),
                None => client.clear_activity(),
            });
            match sent {
                Some(Ok(())) => dirty = false,
                Some(Err(e)) => {
                    tracing::debug!(error = %e, "discord went away; will retry");
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
fn connect() -> Option<DiscordIpcClient> {
    let mut client = DiscordIpcClient::new(DEFAULT_CLIENT_ID);
    match client.connect() {
        Ok(()) => {
            tracing::debug!("connected to Discord");
            Some(client)
        }
        Err(e) => {
            tracing::debug!(error = %e, "Discord is not reachable yet");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence() -> Presence {
        Presence {
            youtube_id: Some("dQw4w9WgXcQ".into()),
            title: "Never Gonna Give You Up".into(),
            artist: "Rick Astley".into(),
            album: Some("Whenever You Need Somebody".into()),
            thumbnail: Some("https://lh3.googleusercontent.com/abc=w120-h120-l90-rj".into()),
            wide: false,
            playing: true,
            end_unix: Some(1_700_000_000),
        }
    }

    /// The activity is built from the presence field for field: listening, the song as details,
    /// the artist as state, the video's own thumbnail, and a watch button. Serializing it back out
    /// is the only way to read the crate's write-only builder.
    #[test]
    fn a_playing_track_becomes_the_expected_activity() {
        let json = serde_json::to_value(presence().activity()).unwrap();
        assert_eq!(json["type"], 2, "Listening");
        assert_eq!(json["name"], "YouTube Music via Moosik");
        assert_eq!(json["details"], "Never Gonna Give You Up");
        assert_eq!(json["state"], "Rick Astley");
        assert_eq!(
            json["status_display_type"], 1,
            "the artist, not the app or the song"
        );
        assert_eq!(json["timestamps"]["end"], 1_700_000_000_000i64);
        // The model's square cover, rewritten to the size Discord draws — not the `i.ytimg`
        // still, whose art-track letterboxing survives Discord's centre crop as bars.
        assert_eq!(
            json["assets"]["large_image"],
            "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj"
        );
        assert_eq!(json["assets"]["large_text"], "Never Gonna Give You Up");
        assert_eq!(
            json["buttons"][0]["url"],
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ"
        );
    }

    /// Paused means no timestamps at all — the bar must not keep counting while the track is
    /// stopped. Everything else stays, so the song is still shown.
    #[test]
    fn a_paused_track_carries_no_progress_bar() {
        let paused = Presence {
            playing: false,
            end_unix: None,
            ..presence()
        };
        let json = serde_json::to_value(paused.activity()).unwrap();
        assert!(json.get("timestamps").is_none());
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
            youtube_id: None,
            thumbnail: Some("x".repeat(URL_LIMIT + 1)),
            ..presence()
        };
        let json = serde_json::to_value(long.activity()).unwrap();
        assert!(json.get("assets").is_none());
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
