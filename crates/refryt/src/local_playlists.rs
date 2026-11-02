//! Private collections persisted locally: legacy file lists, mixed song lists and smart rules.
//! Streamed songs store metadata, not audio or account credentials. Older path-only files
//! remain readable; new fields default to empty and writes use an atomic replacement.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalPlaylist {
    /// `"local:<token>"` for file lists or `"private:<token>"` for private collections; a `PlaylistRef` built from this
    /// carries as its `id`, and how `App::load_playlist` tells a local playlist apart from a
    /// YouTube one.
    pub id: String,
    pub title: String,
    /// In order. Reordering a local playlist is just reordering this `Vec` directly — there is
    /// no server-issued playlist-item id to track, unlike a YouTube playlist's `set_video_id`.
    #[serde(default)]
    pub track_paths: Vec<PathBuf>,
    #[serde(default)]
    pub tracks: Vec<crate::listening::Metadata>,
    #[serde(default)]
    pub rule: Option<SmartRule>,
}

/// A process-local counter, folded into [`new_id`] alongside the clock so two playlists created
/// in the same nanosecond (plausible — creation is a plain synchronous call, not a network round
/// trip) still get distinct ids.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh, unique-enough local playlist id. Not cryptographic — nothing here needs it to be,
/// only unique among playlists this app itself creates.
pub fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("local:{nanos:x}-{seq:x}")
}

fn path() -> Result<std::path::PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "refryt")
        .context("could not find the config directory")?;
    Ok(dirs.config_dir().join("local_playlists.json"))
}

/// Load saved local playlists, falling back to an empty list for anything unreadable — a broken
/// file should not stop the app starting, same reasoning as `settings::load`.
pub fn load() -> Vec<LocalPlaylist> {
    let Ok(path) = path() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "local playlists file could not be parsed; starting empty");
        Vec::new()
    })
}

pub fn save(playlists: &[LocalPlaylist]) -> Result<()> {
    let path = path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(playlists).context("could not encode local playlists")?;
    crate::persist::write_json_atomic(&path, &json)
        .with_context(|| format!("could not write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_paths_and_private_metadata_round_trip() {
        let legacy: LocalPlaylist = serde_json::from_str(
            r#"{"id":"local:old","title":"Old","track_paths":["/music/a.flac"]}"#,
        )
        .unwrap();
        assert_eq!(legacy.track_paths, vec![PathBuf::from("/music/a.flac")]);
        assert!(legacy.tracks.is_empty());
        let mut private = LocalPlaylist {
            id: "private:new".into(),
            title: "Finds".into(),
            track_paths: vec![],
            tracks: vec![],
            rule: None,
        };
        let a = crate::listening::Metadata {
            id: "streamed-song".into(),
            title: "A song".into(),
            artists: vec![("Artist".into(), Some("artist-id".into()))],
            artwork: Some("https://example.com/art.jpg".into()),
            ..Default::default()
        }
        .to_track();
        let b = crate::listening::Metadata {
            id: "/music/b.flac".into(),
            title: "Local song".into(),
            ..Default::default()
        }
        .to_track();
        assert_eq!(private.add_tracks(&[a.clone(), b.clone(), a.clone()]), 2);
        assert_eq!(private.add_tracks(std::slice::from_ref(&a)), 0);
        let lists: Vec<LocalPlaylist> =
            serde_json::from_slice(&serde_json::to_vec(&vec![legacy, private.clone()]).unwrap())
                .unwrap();
        assert_eq!(lists[1], private);
        assert_eq!(
            lists[1]
                .tracks
                .iter()
                .map(|m| m.to_track())
                .collect::<Vec<_>>(),
            vec![a, b]
        );
    }
    #[test]
    fn smart_rules_respect_thresholds_and_order() {
        use crate::listening::{Metadata, Row};
        let now = 100 * 86_400_000;
        let cutoff = now - 30 * 86_400_000;
        let row = |id: &str, first, last, plays, rating| Row {
            id: id.into(),
            first,
            last,
            plays,
            rating,
            listen_ms: 60_000.0,
            metadata: Some(Metadata {
                id: id.into(),
                title: id.into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let rows = [
            row("old", 1, cutoff - 1, 4, 0),
            row("boundary", 1, cutoff, 3, 5),
            row("new", cutoff, now, 1, 5),
            row("rare", 1, 2, 2, 0),
        ];
        let ids = |r: SmartRule| {
            r.matching_tracks(&rows, now)
                .into_iter()
                .map(|t| t.video_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(SmartRule::Forgotten), ["old"]);
        assert_eq!(ids(SmartRule::FiveStars), ["boundary", "new"]);
        assert_eq!(ids(SmartRule::Discoveries), ["new"]);
    }

    #[test]
    fn ids_from_the_same_instant_are_still_distinct() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert!(a.starts_with("local:"));
    }
}

/// Rules operate on this installation's listening ledger, never inferred account history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmartRule {
    Forgotten,
    FiveStars,
    Discoveries,
}
impl SmartRule {
    pub const ALL: [Self; 3] = [Self::Forgotten, Self::FiveStars, Self::Discoveries];
    pub fn title(self) -> &'static str {
        match self {
            Self::Forgotten => "Forgotten favorites",
            Self::FiveStars => "Five-star songs",
            Self::Discoveries => "Recent discoveries",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Forgotten => "Played at least 3 times, unheard for 30 days",
            Self::FiveStars => "Songs you've rated five stars in listening stats",
            Self::Discoveries => "Songs first heard in Moosik in the last 30 days",
        }
    }
    pub fn tracks(self, report: &crate::listening::Report) -> Vec<crate::model::Track> {
        self.matching_tracks(&report.tracks, report.generated_ms)
    }
    fn matching_tracks(
        self,
        tracks: &[crate::listening::Row],
        now: i64,
    ) -> Vec<crate::model::Track> {
        let cutoff = now - 30 * 86_400_000;
        let mut rows: Vec<_> = tracks
            .iter()
            .filter(|r| match self {
                Self::Forgotten => r.plays >= 3 && r.last < cutoff,
                Self::FiveStars => r.rating == 5,
                Self::Discoveries => r.first >= cutoff && r.listen_ms > 0.0,
            })
            .collect();
        rows.sort_by(|a, b| match self {
            Self::Forgotten | Self::FiveStars => b.plays.cmp(&a.plays).then(a.last.cmp(&b.last)),
            Self::Discoveries => b.first.cmp(&a.first),
        });
        rows.into_iter()
            .filter_map(|r| r.metadata.as_ref().map(|m| m.to_track()))
            .collect()
    }
}
impl LocalPlaylist {
    pub fn is_private(&self) -> bool {
        self.id.starts_with("private:")
    }
    pub fn add_tracks(&mut self, tracks: &[crate::model::Track]) -> usize {
        let before = self.tracks.len();
        for track in tracks {
            if !self.tracks.iter().any(|t| t.id == track.video_id) {
                self.tracks
                    .push(crate::listening::Metadata::from_track(track));
            }
        }
        self.tracks.len() - before
    }
}
