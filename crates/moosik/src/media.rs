//! OS media controls and media keys.
//!
//! `playwire` owns the platform backends (MPRIS on Linux, SMTC on Windows, Now Playing on
//! macOS); this module holds the single handle, the queue its callbacks arrive on, and the
//! mapping to the app's own types. Events arrive on a backend thread (a D-Bus worker on Linux),
//! so they are queued and drained once per frame exactly like every other worker reply.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use playwire::{MediaControls, PlaybackState, PlayerConfig, Track};

pub struct Media {
    controls: MediaControls,
    events: Arc<Mutex<Vec<playwire::Event>>>,
}

impl Drop for Media {
    /// One line for the exit log, like `Backend`'s and `Player`'s: the OS media service's
    /// handle is torn down here, and if a quit drags with this as the last line, that teardown
    /// is the one to bound.
    fn drop(&mut self) {
        tracing::info!("media: dropping");
    }
}

impl Media {
    /// Register with the platform's media service. `None` when none is available — no session
    /// bus, an unsupported platform, or a name already claimed — which is not an error: the app
    /// simply runs without OS media controls.
    pub fn new(ctx: egui::Context, hwnd: Option<u64>) -> Option<Self> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let mut config = PlayerConfig::new("Moosik").desktop_entry(crate::flatpak::app_id());
        if let Some(hwnd) = hwnd {
            config = config.hwnd(hwnd);
        }
        let controls = MediaControls::new(config, move |event| {
            if let Ok(mut queue) = sink.lock() {
                queue.push(event);
            }
            ctx.request_repaint();
        })
        .ok()?;
        Some(Self { controls, events })
    }

    /// Take everything the OS sent since the last frame.
    pub fn drain(&self) -> Vec<playwire::Event> {
        self.events
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default()
    }

    pub fn set_state(&mut self, state: &PlaybackState) {
        // A transient publish failure needs no handling; the next tick retries.
        let _ = self.controls.set_state(state);
    }
}

/// Build the snapshot the OS widget shows. `duration` is mpv's (seconds), used when known; the
/// model's own duration is the fallback so a paused, unloaded track still shows a length.
#[allow(clippy::too_many_arguments)]
pub(crate) fn playback_state(
    track: Option<&crate::model::Track>,
    playing: bool,
    position: f64,
    duration: f64,
    volume: f64,
    repeat: crate::app::RepeatMode,
    shuffle: bool,
    can_previous: bool,
    can_next: bool,
) -> PlaybackState {
    let meta = track.map(|t| Track {
        id: t.video_id.clone(),
        title: t.title.clone(),
        artists: t.artists.iter().map(|a| a.name.clone()).collect(),
        album: t.album.clone().unwrap_or_default(),
        artwork_url: t
            .thumbnail
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_default(),
        url: crate::model::track_url(&t.video_id),
    });
    let duration = if duration > 0.0 {
        Some(Duration::from_secs_f64(duration))
    } else {
        track.and_then(|t| t.duration).map(Duration::from_secs)
    };
    PlaybackState {
        track: meta,
        playing,
        position: Duration::from_secs_f64(position.max(0.0)),
        duration,
        volume,
        repeat: match repeat {
            crate::app::RepeatMode::Off => playwire::Repeat::Off,
            crate::app::RepeatMode::All => playwire::Repeat::All,
            crate::app::RepeatMode::One => playwire::Repeat::One,
        },
        shuffle,
        capabilities: playwire::Capabilities {
            can_go_next: can_next,
            can_go_previous: can_previous,
            can_seek: duration.is_some(),
        },
    }
}
