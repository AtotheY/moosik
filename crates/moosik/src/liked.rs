//! Every song the account has liked, indexed by artist.
//!
//! There is no liked-by-artist endpoint. "Liked Music" is one flat playlist — this account's is
//! thousands of rows — while the artist page's own "From your library" shelf is *library*
//! membership: saved albums and the odd bookmarked song, not likes. Measured live on 2026-09-16
//! against a full page-through of Liked Music: The Weeknd had 93 liked songs and that shelf
//! listed 2; Yeat 69 and 10; Kanye West 155 and none at all. The only honest answer to "the songs
//! I liked by this artist" is to read the whole playlist, so the app pages it once per session in
//! the background and keeps the result here — cached to disk, so the next launch has the counts
//! before the scan even starts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{LikeStatus, Track};
use crate::playback::PersistedTrack;

/// How long a complete full scan is trusted before the next launch pages the whole playlist
/// again. In between, every launch refreshes the first page (the newest likes), so a new like
/// shows up at once; an unlike made outside the app can linger until the next full scan.
pub const MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);

/// The liked songs, in the playlist's own (recently-saved) order, with the lookups the artist
/// page and `reconcile_state` need.
#[derive(Default)]
pub struct Index {
    tracks: Vec<Track>,
    by_video: HashMap<String, usize>,
    /// Artist id -> indices into `tracks`. A track credited to several artists is indexed under
    /// each, so an artist page finds it whichever name leads the credit.
    by_artist: HashMap<String, Vec<usize>>,
    /// When a complete full scan last finished, in Unix seconds. `None` until one has.
    scanned_at: Option<u64>,
}

impl Index {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Whether the whole playlist needs paging again — no scan yet, or the last one is older
    /// than [`MAX_AGE`].
    pub fn needs_full_scan(&self) -> bool {
        match self.scanned_at {
            None => true,
            Some(at) => now_secs().saturating_sub(at) > MAX_AGE.as_secs(),
        }
    }

    /// A locked-in answer for the player bar's backfill: `Liked` when the id is in the index.
    pub fn like_of(&self, video_id: &str) -> Option<LikeStatus> {
        self.by_video
            .contains_key(video_id)
            .then_some(LikeStatus::Like)
    }

    pub fn count_for_artist(&self, artist_id: &str) -> usize {
        self.by_artist.get(artist_id).map_or(0, Vec::len)
    }

    /// One artist's liked songs, in the served order. Cloned per call — the page draws from a
    /// snapshot the borrow checker allows, and an artist's slice is small.
    pub fn tracks_for_artist(&self, artist_id: &str) -> Vec<Track> {
        self.by_artist
            .get(artist_id)
            .map(|ixs| ixs.iter().map(|i| self.tracks[*i].clone()).collect())
            .unwrap_or_default()
    }

    /// Add or refresh pages without dropping anything — what the per-launch first-page refresh
    /// uses, so a like made on the phone shows up here without waiting for the next full scan.
    ///
    /// The refreshed page is the playlist's newest slice, in served order, so it goes to the
    /// front: appending would leave "Default" order wrong after the first refresh. A refreshed
    /// copy replaces the old one; everything the page does not mention keeps its relative order
    /// behind it.
    pub fn merge(&mut self, fresh: Vec<Track>) {
        let scanned_at = self.scanned_at;
        let mut seen = std::collections::HashSet::with_capacity(fresh.len());
        let mut order: Vec<Track> = Vec::with_capacity(fresh.len() + self.tracks.len());
        for track in fresh {
            if seen.insert(track.video_id.clone()) {
                order.push(track);
            }
        }
        order.extend(
            self.tracks
                .drain(..)
                .filter(|t| !seen.contains(&t.video_id)),
        );
        *self = Index {
            tracks: order,
            scanned_at,
            ..Index::default()
        };
        self.rebuild_maps();
    }

    /// Wholesale replacement from a completed full scan: anything no longer in the playlist is
    /// gone, duplicates collapse, and the scan is stamped as fresh.
    pub fn replace(&mut self, fresh: Vec<Track>) {
        let mut index = Index::default();
        for track in fresh {
            match index.by_video.get(&track.video_id).copied() {
                Some(i) => index.tracks[i] = track,
                None => index.push(track),
            }
        }
        index.scanned_at = Some(now_secs());
        *self = index;
    }

    /// A song the listener just liked, straight from the rating action. A new like is the newest
    /// like, so it lands at the front of the served order; an existing entry updates in place.
    pub fn set_liked(&mut self, track: Track) {
        if let Some(i) = self.by_video.get(&track.video_id).copied() {
            self.tracks[i] = track;
            return;
        }
        self.tracks.insert(0, track);
        self.rebuild_maps();
    }

    /// A song the listener just unliked. The indices after it shift, so the maps are rebuilt
    /// rather than patched — a few thousand entries, and unlikes are rare.
    pub fn set_unliked(&mut self, video_id: &str) {
        let Some(i) = self.by_video.get(video_id).copied() else {
            return;
        };
        self.tracks.remove(i);
        self.rebuild_maps();
    }

    fn push(&mut self, track: Track) {
        let i = self.tracks.len();
        for artist in &track.artists {
            if let Some(id) = &artist.id {
                self.by_artist.entry(id.clone()).or_default().push(i);
            }
        }
        self.by_video.insert(track.video_id.clone(), i);
        self.tracks.push(track);
    }

    fn rebuild_maps(&mut self) {
        self.by_video.clear();
        self.by_artist.clear();
        for (i, track) in self.tracks.iter().enumerate() {
            for artist in &track.artists {
                if let Some(id) = &artist.id {
                    self.by_artist.entry(id.clone()).or_default().push(i);
                }
            }
            self.by_video.insert(track.video_id.clone(), i);
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn path() -> Result<PathBuf> {
    let dirs = crate::profile_dirs().context("could not find the config directory")?;
    Ok(dirs.config_dir().join("liked.json"))
}

#[derive(Serialize, Deserialize)]
struct Store {
    version: u32,
    #[serde(default)]
    scanned_at: Option<u64>,
    tracks: Vec<PersistedTrack>,
}

/// The cached index, or an empty one when there is no cache (or it cannot be read: a corrupt
/// cache is simply no cache, and the next scan rebuilds it).
pub fn load() -> Index {
    let Ok(path) = path() else {
        return Index::default();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Index::default();
    };
    let Ok(store) = serde_json::from_slice::<Store>(&bytes) else {
        return Index::default();
    };
    let mut index = Index {
        scanned_at: store.scanned_at,
        ..Index::default()
    };
    for persisted in store.tracks {
        let mut track = persisted.into_track();
        // Everything in this file is liked by definition; the store carries no rating of its own.
        track.like = LikeStatus::Like;
        index.push(track);
    }
    index
}

pub fn save(index: &Index) -> Result<()> {
    let path = path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let store = Store {
        version: 1,
        scanned_at: index.scanned_at,
        tracks: index
            .tracks
            .iter()
            .map(PersistedTrack::from_track)
            .collect(),
    };
    let bytes = serde_json::to_vec(&store).context("could not encode the liked index")?;
    crate::persist::write_json_atomic(&path, &bytes)
        .with_context(|| format!("could not write {}", path.display()))
}

/// Drop the cache — account data, so sign-out removes it rather than leaving one account's
/// likes on disk for the next.
pub fn remove() {
    if let Ok(path) = path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ArtistLink;

    fn track(id: &str, artists: &[(&str, Option<&str>)]) -> Track {
        Track {
            video_id: id.to_owned(),
            title: id.to_owned(),
            artists: artists
                .iter()
                .map(|(name, id)| ArtistLink {
                    name: (*name).to_owned(),
                    id: id.map(str::to_owned),
                })
                .collect(),
            album: None,
            album_id: None,
            duration: None,
            plays: None,
            explicit: false,
            like: LikeStatus::Like,
            in_library: false,
            library_add: None,
            library_remove: None,
            set_video_id: None,
            thumbnail: None,
            wide: false,
            rank: None,
            trend: crate::model::ChartTrend::Unknown,
            local_path: None,
        }
    }

    #[test]
    fn an_artist_sees_their_own_liked_songs() {
        let mut index = Index::default();
        index.merge(vec![
            track("a", &[("The Weeknd", Some("UCw"))]),
            track("b", &[("The Weeknd", Some("UCw")), ("Ariana", Some("UCa"))]),
            track("c", &[("Someone", Some("UCs"))]),
        ]);
        assert_eq!(index.count_for_artist("UCw"), 2);
        assert_eq!(
            index.count_for_artist("UCa"),
            1,
            "featured credits count too"
        );
        assert_eq!(index.tracks_for_artist("UCw").len(), 2);
        assert_eq!(index.like_of("b"), Some(LikeStatus::Like));
        assert_eq!(index.like_of("zz"), None);
    }

    #[test]
    fn merging_refreshes_without_dropping_or_duplicating() {
        let mut index = Index::default();
        index.merge(vec![track("a", &[("X", Some("UCx"))])]);
        let mut updated = track("a", &[("X", Some("UCx"))]);
        updated.title = "renamed".to_owned();
        index.merge(vec![updated, track("b", &[("X", Some("UCx"))])]);
        assert_eq!(
            index.count_for_artist("UCx"),
            2,
            "no duplicate for the same id"
        );
        assert_eq!(index.tracks_for_artist("UCx")[0].title, "renamed");
    }

    #[test]
    fn a_full_scan_replaces_and_stamps() {
        let mut index = Index::default();
        index.merge(vec![track("a", &[("X", Some("UCx"))])]);
        assert!(index.needs_full_scan(), "never scanned");
        index.replace(vec![
            track("b", &[("X", Some("UCx"))]),
            track("b", &[("X", Some("UCx"))]),
        ]);
        assert_eq!(index.count_for_artist("UCx"), 1, "duplicates collapse");
        assert_eq!(index.like_of("a"), None, "the old page is gone");
        assert!(!index.needs_full_scan());
    }

    #[test]
    fn rating_updates_the_index_both_ways() {
        let mut index = Index::default();
        index.set_liked(track("a", &[("X", Some("UCx"))]));
        assert_eq!(index.like_of("a"), Some(LikeStatus::Like));
        index.set_unliked("a");
        assert_eq!(index.like_of("a"), None);
        assert_eq!(index.count_for_artist("UCx"), 0);
    }
}
