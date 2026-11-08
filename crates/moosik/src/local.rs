//! Local audio files: walking a folder tree and turning what's found into `Track`s.
//!
//! This is the only place that touches the filesystem directly rather than YouTube's API — the
//! scan runs on a blocking thread (`Backend::scan_local_folders` hands it to
//! `tokio::task::spawn_blocking`) since directory walking and tag parsing are ordinary
//! synchronous I/O, not the async HTTP calls everything else in `backend.rs` makes.

use std::path::Path;
use std::sync::Arc;

use crate::model::{ArtistLink, ChartTrend, LikeStatus, Track};

/// Allow-listed rather than deny-listed: an unrecognised extension is far more likely to be
/// artwork, a playlist file or a stray document sitting in a music folder than a format lofty
/// and mpv both happen to not support.
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "flac", "ogg", "oga", "m4a", "aac", "opus", "wma",
];

/// Walks every root recursively and returns one `Track` per audio file found. Best-effort: a
/// file lofty can't read its tags for still becomes a track (titled from its filename, with no
/// duration) rather than being dropped, and a directory entry `walkdir` can't read (permissions,
/// a broken symlink) is skipped rather than failing the whole scan.
pub fn scan(roots: &[String]) -> Vec<Track> {
    roots
        .iter()
        .flat_map(|root| {
            walkdir::WalkDir::new(root)
                .into_iter()
                .filter_map(|entry| match entry {
                    Ok(entry) => Some(entry),
                    Err(e) => {
                        tracing::debug!(error = %e, "could not read a directory entry while scanning local music");
                        None
                    }
                })
                .filter(|entry| entry.file_type().is_file() && is_audio(entry.path()))
                .filter_map(|entry| track_from_path(entry.path()))
        })
        .collect()
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(ext)))
}

fn track_from_path(path: &Path) -> Option<Track> {
    // Canonicalized so `video_id` — every other track's stable identity key, reused here rather
    // than invented — never depends on which of two equivalent paths (a symlink, `.`/`..`) the
    // walk happened to hand back.
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| canonical.to_string_lossy().into_owned());

    let (title, artist, album, duration) = match lofty::probe::read_from_path(&canonical) {
        Ok(tagged) => {
            use lofty::prelude::*;
            let tag = tagged.primary_tag();
            let non_empty = |c: std::borrow::Cow<'_, str>| {
                let s = c.into_owned();
                (!s.trim().is_empty()).then_some(s)
            };
            (
                tag.and_then(|t| t.title()).and_then(non_empty),
                tag.and_then(|t| t.artist()).and_then(non_empty),
                tag.and_then(|t| t.album()).and_then(non_empty),
                Some(tagged.properties().duration().as_secs()),
            )
        }
        Err(e) => {
            tracing::debug!(
                path = %canonical.display(),
                error = %e,
                "could not read tags, falling back to the filename"
            );
            (None, None, None, None)
        }
    };

    Some(Track {
        video_id: canonical.to_string_lossy().into_owned(),
        title: title.unwrap_or(stem),
        artists: artist
            .map(|a| vec![ArtistLink::plain(a)])
            .unwrap_or_default(),
        album,
        album_id: None,
        duration,
        plays: None,
        explicit: false,
        like: LikeStatus::Indifferent,
        in_library: false,
        library_add: None,
        library_remove: None,
        set_video_id: None,
        // No embedded-art extraction yet — the image pipeline (`images.rs`) is built entirely
        // around YouTube's `sized_url` template, which a local path can't go through. Local
        // tracks show the generic placeholder cover until that gets its own pass.
        thumbnail: None,
        wide: false,
        rank: None,
        trend: ChartTrend::Unknown,
        local_path: Some(Arc::from(canonical.as_path())),
    })
}

pub fn search(tracks: &[crate::model::Track], query: &str) -> crate::model::SearchResults {
    let query = query.to_lowercase();
    let terms: Vec<_> = query.split_whitespace().collect();
    let items = if terms.is_empty() {
        Vec::new()
    } else {
        let mut text = String::new();
        tracks
            .iter()
            .filter(|track| {
                text.clear();
                text.push_str(&track.title);
                for field in track
                    .artists
                    .iter()
                    .map(|artist| artist.name.as_str())
                    .chain(track.album.as_deref())
                {
                    text.push(' ');
                    text.push_str(field);
                }
                if text.is_ascii() {
                    text.make_ascii_lowercase();
                    terms.iter().all(|term| text.contains(term))
                } else {
                    let lowercase = text.to_lowercase();
                    terms.iter().all(|term| lowercase.contains(term))
                }
            })
            .cloned()
            .map(crate::model::Entry::Song)
            .collect::<Vec<_>>()
    };
    crate::model::SearchResults {
        no_results: items.is_empty(),
        items,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn local_search_matches_words_across_title_artist_and_album() {
        let mut track = super::track_from_path(std::path::Path::new("One More Time.flac")).unwrap();
        track.artists = vec![crate::model::ArtistLink::plain("Daft Punk")];
        track.album = Some("Discovery".to_owned());
        let tracks = vec![track];
        let found = super::search(&tracks, "DAFT discovery time");
        assert_eq!(found.items.len(), 1);
        assert_eq!(
            found.items[0].track().unwrap().provider(),
            crate::model::Provider::Local
        );
        assert!(super::search(&tracks, "missing").no_results);
        assert!(super::search(&tracks, "  ").items.is_empty());
    }

    use super::*;

    /// Not a real audio file — lofty will refuse to read tags from it — but the scan still has
    /// to produce a track, titled from the filename, rather than dropping the file entirely.
    #[test]
    fn a_file_lofty_cannot_read_still_becomes_a_track_titled_from_its_name() {
        let dir = std::env::temp_dir().join(format!("moosik-local-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Not Actually Audio.mp3");
        std::fs::write(&file, b"not a real mp3").unwrap();

        let tracks = scan(&[dir.to_string_lossy().into_owned()]);
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Not Actually Audio");
        assert!(tracks[0].local_path.is_some());
        assert!(tracks[0].duration.is_none());
    }

    /// Allow-listed, not deny-listed: cover art and stray text files sitting next to the music
    /// must not turn into bogus rows.
    #[test]
    fn non_audio_files_are_skipped() {
        let dir = std::env::temp_dir().join(format!("moosik-local-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cover.jpg"), b"not audio").unwrap();
        std::fs::write(dir.join("notes.txt"), b"not audio").unwrap();

        let tracks = scan(&[dir.to_string_lossy().into_owned()]);
        std::fs::remove_dir_all(&dir).ok();

        assert!(tracks.is_empty());
    }

    /// A file nested a couple of folders deep is still found — the whole point of walking
    /// recursively rather than reading just the top level.
    #[test]
    fn nested_folders_are_walked_recursively() {
        let dir = std::env::temp_dir().join(format!("moosik-local-test3-{}", std::process::id()));
        let nested = dir.join("Rock").join("Album");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("track.flac"), b"not real either").unwrap();

        let tracks = scan(&[dir.to_string_lossy().into_owned()]);
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "track");
    }
}
