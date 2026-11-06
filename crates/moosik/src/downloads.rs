//! Offline downloads.
//!
//! A downloaded song is a file under `~/Music/moosik` plus a record in `downloads.json` that maps
//! its video id to the file and remembers the metadata. The backend consults the index before
//! resolving, so a downloaded track plays from disk — offline, and without the resolver — while
//! still scrobbling like any other play. Metadata is written into the file with `lofty` (no
//! ffmpeg needed); the cover is fetched and embedded the same way.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::UserDirs;
use serde::{Deserialize, Serialize};

use crate::model::Track;
use crate::playback::PersistedTrack;

/// Where downloads live: `~/Music/moosik`, or the data dir if there is no XDG music folder.
pub fn dir() -> Result<PathBuf> {
    if let Some(user) = UserDirs::new()
        && let Some(music) = user.audio_dir()
    {
        let current = music.join("moosik");
        let legacy = music.join("refryt");
        return Ok(if !current.exists() && legacy.is_dir() {
            legacy
        } else {
            current
        });
    }
    let dirs = crate::profile_dirs().context("could not find a data directory")?;
    Ok(dirs.data_dir().join("downloads"))
}

fn index_path() -> Result<PathBuf> {
    let dirs = crate::profile_dirs().context("could not find the config directory")?;
    Ok(dirs.config_dir().join("downloads.json"))
}

/// One downloaded track: where the file is, and enough to build a `Track` back.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub path: String,
    #[serde(flatten)]
    pub track: PersistedTrack,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Index {
    #[serde(default)]
    pub entries: HashMap<String, Record>,
}

impl Index {
    /// The file for a video id, if it is downloaded.
    pub fn path_for(&self, video_id: &str) -> Option<&Path> {
        self.entries.get(video_id).map(|r| Path::new(&r.path))
    }

    /// Point entries whose file moved — or whose path was recorded before the printed-path bug
    /// was understood — back at the file actually on disk. In memory only; the next save
    /// persists it. Takes the directory so the repair is testable without touching the real
    /// `~/Music/moosik`.
    /// Whether anything changed.
    pub(crate) fn repair_paths(&mut self, dir: &Path) -> bool {
        let mut changed = false;
        for (id, record) in &mut self.entries {
            if Path::new(&record.path).is_file() {
                continue;
            }
            if let Some(found) = find_downloaded(dir, id) {
                record.path = found.to_string_lossy().into_owned();
                changed = true;
            }
        }
        changed
    }

    /// The downloaded tracks, for the Library's Downloads list.
    pub fn tracks(&self) -> Vec<Track> {
        self.entries
            .values()
            .map(|r| r.track.clone().into_track())
            .collect()
    }
}

pub fn load() -> Index {
    let Some(path) = index_path().ok() else {
        return Index::default();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Index::default();
    };
    let mut index: Index = serde_json::from_slice(&bytes).unwrap_or_default();
    if let Ok(dir) = dir()
        && index.repair_paths(&dir)
    {
        // Persist the repair: otherwise every launch rescans, and Remove download keeps aiming
        // at a path that is not there.
        let _ = save(&index);
    }
    index
}

pub fn save(index: &Index) -> Result<()> {
    let path = index_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(index).context("could not encode the download index")?;
    crate::persist::write_json_atomic(&path, &bytes)
        .with_context(|| format!("could not write {}", path.display()))
}

/// The yt-dlp arguments for one track: the audio format and a per-track output template that
/// cannot collide.
///
/// `--no-playlist` always: an album or playlist is downloaded by iterating its tracks, so a
/// single invocation is ever only one song.
///
/// No `--print after_move:filepath`: yt-dlp's stdout is not a safe channel for a path on
/// Windows. Its own sanitization turns a `:` into `：`, and that character does not survive the
/// pipe — PyInstaller's stream drops it whatever the Python encoding says, so the recorded path
/// pointed at a file that did not exist (`find_downloaded` is how the file is located instead).
pub fn yt_dlp_args(track: &Track, format: &str, dir: &Path) -> Vec<String> {
    let template = dir.join("%(title)s [%(id)s].%(ext)s");
    vec![
        "--no-playlist".into(),
        "--no-warnings".into(),
        "--newline".into(),
        "-f".into(),
        format.to_owned(),
        "-o".into(),
        template.to_string_lossy().into_owned(),
        crate::model::track_url(&track.video_id),
    ]
}

/// The file `yt-dlp` just wrote for `video_id` under `dir`.
///
/// The output template puts the id in brackets, so this is the one thing in the name that is
/// ours and stable — the title can be anything, and its sanitized form is not something to
/// reconstruct from stdout. Newest match wins, so a re-download supersedes the old file.
pub fn find_downloaded(dir: &Path, video_id: &str) -> Option<PathBuf> {
    let marker = format!("[{video_id}]");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name();
        if !name.to_string_lossy().contains(&marker) {
            continue;
        }
        let path = entry.path();
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(when, _)| modified > *when) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, path)| path)
}

/// Write the tags and cover into a downloaded file. Best-effort: a format `lofty` cannot tag
/// still plays, it just sits under its filename in other players.
pub fn embed_tags(path: &Path, track: &Track, cover: Option<&[u8]>) {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::prelude::{Accessor, TagExt};

    let Ok(mut tagged) = lofty::probe::read_from_path(path) else {
        return;
    };
    if tagged.primary_tag_mut().is_none() && tagged.first_tag_mut().is_none() {
        return;
    }
    let tag = match tagged.primary_tag_mut() {
        Some(tag) => tag,
        None => match tagged.first_tag_mut() {
            Some(tag) => tag,
            None => return,
        },
    };
    tag.set_title(track.title.clone());
    if !track.artists.is_empty() {
        tag.set_artist(track.artist_line());
    }
    if let Some(album) = &track.album {
        tag.set_album(album.clone());
    }
    if let Some(cover) = cover {
        use lofty::picture::{MimeType, Picture, PictureType};
        let mime = if cover.starts_with(&[0x89, b'P', b'N', b'G']) {
            MimeType::Png
        } else {
            MimeType::Jpeg
        };
        let picture = Picture::unchecked(cover.to_vec())
            .pic_type(PictureType::CoverFront)
            .mime_type(mime)
            .build();
        tag.set_picture(0, picture);
    }
    let _ = tag.save_to_path(path, WriteOptions::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArtistLink, ChartTrend, LikeStatus};

    fn sample_track() -> Track {
        Track {
            video_id: "9RfVp-GhKfs".into(),
            title: "Creep".into(),
            artists: vec![ArtistLink::plain("Radiohead")],
            album: Some("Creep".into()),
            album_id: None,
            duration: Some(239),
            plays: None,
            explicit: false,
            like: LikeStatus::Indifferent,
            in_library: false,
            library_add: None,
            library_remove: None,
            set_video_id: None,
            wide: false,
            rank: None,
            trend: ChartTrend::Unknown,
            thumbnail: None,
            local_path: None,
        }
    }

    #[test]
    fn the_index_round_trips_and_resolves_paths() {
        let mut index = Index::default();
        index.entries.insert(
            "9RfVp-GhKfs".into(),
            Record {
                path: "/music/Creep.webm".into(),
                track: PersistedTrack::from_track(&sample_track()),
            },
        );
        let json = serde_json::to_string(&index).unwrap();
        let back: Index = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.path_for("9RfVp-GhKfs"),
            Some(Path::new("/music/Creep.webm"))
        );
        assert_eq!(back.path_for("nope"), None);
        let tracks = back.tracks();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Creep");
    }

    #[test]
    fn the_yt_dlp_args_are_single_track_and_marked_with_the_id() {
        let args = yt_dlp_args(
            &sample_track(),
            "bestaudio[acodec=opus]/bestaudio",
            Path::new("/tmp/dl"),
        );
        assert!(args.iter().any(|a| a == "--no-playlist"));
        assert!(
            !args.iter().any(|a| a == "--print"),
            "the printed path is not trustworthy; the file is found by its id marker instead"
        );
        assert!(args.iter().any(|a| a.contains("[%(id)s]")));
        assert!(
            args.last()
                .is_some_and(|a| a.contains("watch?v=9RfVp-GhKfs"))
        );
    }

    /// The printed path is not trustworthy on Windows, so the file is found by the one stable
    /// part of the name: the id in brackets from the output template.
    #[test]
    fn finds_the_download_by_its_id_marker() {
        let dir = std::env::temp_dir().join(format!("moosik-dl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Something else [nope].webm"), b"x").unwrap();
        std::fs::write(
            dir.join("Macacoa 2000 (from GTAVI： The Album) [DShU-ly9BdM].webm"),
            b"x",
        )
        .unwrap();
        let found = find_downloaded(&dir, "DShU-ly9BdM").expect("the matching file");
        assert_eq!(
            found.file_name().unwrap(),
            "Macacoa 2000 (from GTAVI： The Album) [DShU-ly9BdM].webm"
        );
        assert!(find_downloaded(&dir, "missing").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A recorded path that no longer matches the file on disk is healed at load, so a download
    /// recorded through the old stdout path cannot become unplayable.
    #[test]
    fn a_stale_path_is_repaired_from_the_id_marker() {
        let dir = std::env::temp_dir().join(format!("moosik-dl-repair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Macacoa 2000 (from GTAVI： The Album) [DShU-ly9BdM].webm"),
            b"x",
        )
        .unwrap();
        let mut index = Index::default();
        index.entries.insert(
            "DShU-ly9BdM".into(),
            Record {
                path: dir
                    .join("Macacoa 2000 (from GTAVI The Album) [DShU-ly9BdM].webm")
                    .to_string_lossy()
                    .into_owned(),
                track: PersistedTrack::from_track(&sample_track()),
            },
        );
        assert!(index.repair_paths(&dir), "the stale path must be repaired");
        assert!(Path::new(&index.entries["DShU-ly9BdM"].path).is_file());
        assert!(
            !index.repair_paths(&dir),
            "a repaired index needs no second repair"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
