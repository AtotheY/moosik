//! Fill missing listening genres in the background. Local files use their embedded tag;
//! online tracks send only title and artist to Apple's song catalog for a match.
//! Play counts, times, and the listening database are never sent.
use super::{Metadata, Reply, now_ms};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const MISS_RETRY_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const ERROR_RETRY_MS: i64 = 10 * 60 * 1000;
const SEARCH_GAP: Duration = Duration::from_millis(3100); // Apple's ~20 requests/minute guidance.
const DB_LOCK_RETRY_GAP: Duration = Duration::from_millis(250);

pub(super) struct Job {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Job {
    pub(super) fn start(path: PathBuf, replies: Sender<Reply>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = thread::Builder::new()
            .name("moosik-genres".into())
            .spawn(move || {
                if let Err(error) = run(&path, &replies, &stopped) {
                    tracing::warn!(%error, "genre lookup stopped");
                }
            })
            .map_err(|error| tracing::warn!(%error, "could not start genre lookup"))
            .ok();
        Self { stop, thread }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // A removable or network-backed local file can stall a tag probe. Let the
        // background thread wind down without delaying storage shutdown.
        drop(self.thread.take());
    }
}

fn pause(stop: &AtomicBool, duration: Duration) {
    let until = std::time::Instant::now() + duration;
    while !stop.load(Ordering::Relaxed) && std::time::Instant::now() < until {
        thread::sleep(
            Duration::from_millis(100)
                .min(until.saturating_duration_since(std::time::Instant::now())),
        );
    }
}

fn temporary_db_lock(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(failure, _))
                if matches!(
                    failure.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )
        )
    })
}

// The recorder writes on a separate connection. Its temporary lock must not end
// genre discovery or make us repeat an already completed catalog request.
fn retry_db_lock<T>(
    stop: &AtomicBool,
    mut operation: impl FnMut() -> Result<T>,
) -> Result<Option<T>> {
    let mut reported = false;
    while !stop.load(Ordering::Relaxed) {
        match operation() {
            Ok(value) => return Ok(Some(value)),
            Err(error) if temporary_db_lock(&error) => {
                if !reported {
                    tracing::debug!(%error, "listening database busy; retrying genre lookup operation");
                    reported = true;
                }
                pause(stop, DB_LOCK_RETRY_GAP);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

fn next(db: &Connection) -> Result<Option<Metadata>> {
    let metadata: Option<String> = db
        .query_row(
            "SELECT s.metadata FROM sessions s
             JOIN segments heard ON heard.session_id=s.id AND heard.listen_ms>0
             LEFT JOIN track_tags t ON t.track_id=s.track_id
             LEFT JOIN auto_genres a ON a.track_id=s.track_id
             WHERE COALESCE(t.genres,'')='' AND COALESCE(t.genres_override,0)=0
               AND (a.track_id IS NULL OR
                    (a.genres='' AND a.source!='skipped' AND
                     a.checked_ms < CASE WHEN a.source='error' THEN ?2 ELSE ?1 END))
             ORDER BY s.started_ms DESC LIMIT 1",
            [now_ms() - MISS_RETRY_MS, now_ms() - ERROR_RETRY_MS],
            |row| row.get(0),
        )
        .optional()?;
    metadata
        .map(|json| serde_json::from_str(&json).context("invalid listening track metadata"))
        .transpose()
}

fn save(db: &Connection, track_id: &str, genres: &str, source: &str) -> Result<()> {
    db.execute(
        "INSERT INTO auto_genres(track_id,genres,source,checked_ms) VALUES (?1,?2,?3,?4)
         ON CONFLICT(track_id) DO UPDATE SET genres=excluded.genres,source=excluded.source,checked_ms=excluded.checked_ms",
        params![track_id, genres, source, now_ms()],
    )?;
    Ok(())
}

fn run(path: &Path, replies: &Sender<Reply>, stop: &AtomicBool) -> Result<()> {
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(2))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let client = reqwest::Client::builder()
        .user_agent(concat!(
            "Moosik/",
            env!("CARGO_PKG_VERSION"),
            " (genre lookup)"
        ))
        .timeout(Duration::from_secs(8))
        .build()?;
    while !stop.load(Ordering::Relaxed) {
        let Some(candidate) = retry_db_lock(stop, || next(&db))? else {
            break;
        };
        let Some(track) = candidate else {
            pause(stop, Duration::from_secs(5));
            continue;
        };
        if Path::new(&track.id).is_absolute() {
            let genre = local_genre(&track.id).unwrap_or_default();
            if retry_db_lock(stop, || save(&db, &track.id, &genre, "file"))?.is_none() {
                break;
            }
            if !genre.is_empty() {
                let _ = replies.send(Reply::GenresUpdated);
            }
            continue;
        }
        if !eligible(&track) {
            if retry_db_lock(stop, || save(&db, &track.id, "", "skipped"))?.is_none() {
                break;
            }
            continue;
        }
        match runtime.block_on(lookup(&client, &track)) {
            Ok(genre) => {
                if retry_db_lock(stop, || {
                    save(&db, &track.id, genre.as_deref().unwrap_or(""), "apple")
                })?
                .is_none()
                {
                    break;
                }
                if genre.is_some() {
                    let _ = replies.send(Reply::GenresUpdated);
                }
                pause(stop, SEARCH_GAP);
            }
            Err(error) => {
                tracing::debug!(%error, "genre lookup unavailable; retrying later");
                if retry_db_lock(stop, || save(&db, &track.id, "", "error"))?.is_none() {
                    break;
                }
                pause(stop, Duration::from_secs(60));
            }
        }
    }
    Ok(())
}

fn local_genre(path: &str) -> Option<String> {
    use lofty::prelude::*;
    let audio = lofty::probe::read_from_path(path).ok()?;
    audio
        .primary_tag()
        .or_else(|| audio.first_tag())
        .and_then(|tag| tag.genre())
        .map(|genre| genre.trim().chars().take(80).collect::<String>())
        .filter(|genre| !genre.is_empty())
}

fn eligible(track: &Metadata) -> bool {
    let artist = track.artists.first().map(|a| a.0.trim()).unwrap_or("");
    let valid = |value: &str| {
        normalized(value).chars().count() >= 2
            && !matches!(
                value.to_ascii_lowercase().as_str(),
                "unknown" | "unknown artist" | "untitled" | "various artists"
            )
    };
    !track.id.is_empty() && valid(track.title.trim()) && valid(artist)
}

#[derive(Deserialize)]
struct Search {
    results: Vec<Song>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Song {
    kind: Option<String>,
    track_name: Option<String>,
    artist_name: Option<String>,
    collection_name: Option<String>,
    primary_genre_name: Option<String>,
    track_time_millis: Option<u64>,
}

async fn lookup(client: &reqwest::Client, track: &Metadata) -> Result<Option<String>> {
    let artist = &track.artists[0].0;
    let response = client
        .get("https://itunes.apple.com/search")
        .query(&[
            ("term", format!("{artist} {}", track.title)),
            ("entity", "song".into()),
            ("country", "us".into()),
            ("limit", "25".into()),
        ])
        .send()
        .await?
        .error_for_status()?;
    let bytes = response.bytes().await?;
    let songs: Search = serde_json::from_slice(&bytes)?;
    Ok(best_genre(track, &songs.results))
}

fn normalized(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn title_base(value: &str) -> &str {
    for marker in [" (", " [", " - "] {
        if let Some((base, suffix)) = value.rsplit_once(marker) {
            let suffix = suffix.to_ascii_lowercase();
            if [
                "remaster",
                "official",
                "lyric",
                "visualizer",
                "feat.",
                "featuring",
                "ft.",
            ]
            .iter()
            .any(|word| suffix.contains(word))
            {
                return base;
            }
        }
    }
    value
}

fn artist_matches(expected: &str, actual: &str) -> bool {
    let expected = normalized(expected);
    if expected.is_empty() {
        return false;
    }
    if normalized(actual) == expected {
        return true;
    }
    // Catalog collaborations may append another artist. Never accept a substring match.
    for separator in [" & ", ", ", " feat. ", " featuring ", " x "] {
        if let Some((first, _)) = actual.split_once(separator)
            && normalized(first) == expected
        {
            return true;
        }
    }
    false
}

fn best_genre(track: &Metadata, songs: &[Song]) -> Option<String> {
    let artist = track.artists.first()?.0.as_str();
    let title = normalized(&track.title);
    let base = normalized(title_base(&track.title));
    let album = track.album.as_deref().map(normalized);
    let mut best_score = 0;
    let mut best: Option<&str> = None;
    let mut ambiguous = false;
    for song in songs {
        if song.kind.as_deref() != Some("song")
            || !artist_matches(artist, song.artist_name.as_deref().unwrap_or(""))
        {
            continue;
        }
        let song_title = song.track_name.as_deref().unwrap_or("");
        let score = if normalized(song_title) == title {
            3
        } else if normalized(title_base(song_title)) == base {
            2
        } else {
            continue;
        };
        let genre = song.primary_genre_name.as_deref().unwrap_or("").trim();
        if genre.is_empty() || matches!(genre, "Music" | "Unknown") || genre.chars().count() > 80 {
            continue;
        }
        if !track.video
            && track.duration > 0.0
            && song
                .track_time_millis
                .is_some_and(|ms| (ms as f64 / 1000.0 - track.duration).abs() > 15.0)
        {
            continue;
        }
        let score = score
            + i32::from(album.as_ref().is_some_and(|a| {
                Some(a.as_str()) == song.collection_name.as_deref().map(normalized).as_deref()
            }))
            + i32::from(song.track_time_millis.is_some_and(|ms| {
                track.duration > 0.0 && (ms as f64 / 1000.0 - track.duration).abs() <= 8.0
            }));
        if score > best_score {
            best_score = score;
            best = Some(genre);
            ambiguous = false;
        } else if score == best_score
            && best.is_some_and(|value| !value.eq_ignore_ascii_case(genre))
        {
            ambiguous = true;
        }
    }
    (!ambiguous).then(|| best.map(str::to_owned)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_database_lock_does_not_drop_a_genre_match() {
        use std::sync::atomic::AtomicUsize;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("listening.db");
        let holder = Connection::open(&path).unwrap();
        holder
            .execute_batch(
                "CREATE TABLE auto_genres(track_id TEXT PRIMARY KEY,genres TEXT,source TEXT,checked_ms INTEGER);
                 BEGIN IMMEDIATE;",
            )
            .unwrap();
        let writer = Connection::open(&path).unwrap();
        writer.busy_timeout(Duration::from_millis(10)).unwrap();
        let stop = AtomicBool::new(false);
        let attempts = AtomicUsize::new(0);
        let (first_result, first_attempt) = std::sync::mpsc::channel();

        thread::scope(|scope| {
            let stop = &stop;
            let attempts = &attempts;
            let job = scope.spawn(move || {
                retry_db_lock(stop, || {
                    let result = save(&writer, "track", "Alternative", "apple");
                    if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                        first_result
                            .send(result.as_ref().err().is_some_and(temporary_db_lock))
                            .unwrap();
                    }
                    result
                })
                .unwrap()
            });
            let was_busy = first_attempt.recv().unwrap();
            holder.execute_batch("COMMIT").unwrap();
            assert!(job.join().unwrap().is_some());
            assert!(was_busy);
        });

        assert!(attempts.load(Ordering::Relaxed) >= 2);
        let genre: String = holder
            .query_row(
                "SELECT genres FROM auto_genres WHERE track_id='track'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(genre, "Alternative");
    }

    #[test]
    fn permanent_database_errors_remain_visible() {
        let db = Connection::open_in_memory().unwrap();
        let stop = AtomicBool::new(false);
        assert!(retry_db_lock(&stop, || save(&db, "track", "Rock", "apple")).is_err());
    }

    fn track() -> Metadata {
        Metadata {
            id: "youtube-id".into(),
            title: "No Surprises".into(),
            artists: vec![("Radiohead".into(), None)],
            album: Some("OK Computer".into()),
            duration: 229.0,
            ..Metadata::default()
        }
    }
    fn song(title: &str, artist: &str, album: &str, genre: &str, duration: u64) -> Song {
        Song {
            kind: Some("song".into()),
            track_name: Some(title.into()),
            artist_name: Some(artist.into()),
            collection_name: Some(album.into()),
            primary_genre_name: Some(genre.into()),
            track_time_millis: Some(duration * 1000),
        }
    }
    #[test]
    fn matches_title_artist_album_and_duration_without_guessing() {
        let track = track();
        assert_eq!(
            best_genre(
                &track,
                &[song(
                    "No Surprises",
                    "Radiohead",
                    "OK Computer",
                    "Alternative",
                    229
                )]
            )
            .as_deref(),
            Some("Alternative")
        );
        assert_eq!(
            best_genre(
                &track,
                &[song(
                    "No Surprises",
                    "Another Artist",
                    "OK Computer",
                    "Pop",
                    229
                )]
            ),
            None
        );
        assert_eq!(
            best_genre(
                &track,
                &[song("No Surprises", "Radiohead", "OK Computer", "Pop", 300)]
            ),
            None
        );
        assert_eq!(
            best_genre(
                &track,
                &[song(
                    "No Surprises (Remastered)",
                    "Radiohead",
                    "OK Computer",
                    "Alternative",
                    229
                )]
            )
            .as_deref(),
            Some("Alternative")
        );
        assert_eq!(
            best_genre(
                &track,
                &[
                    song("No Surprises", "Radiohead", "OK Computer", "Pop", 229),
                    song(
                        "No Surprises",
                        "Radiohead",
                        "OK Computer",
                        "Alternative",
                        229
                    )
                ]
            ),
            None
        );
    }
    #[test]
    fn skips_placeholder_metadata() {
        let mut track = track();
        track.artists.clear();
        assert!(!eligible(&track));
        track.artists.push(("Unknown artist".into(), None));
        assert!(!eligible(&track));
    }
    #[test]
    fn caches_hits_and_misses_but_manual_genres_take_priority() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE sessions(id TEXT,track_id TEXT,metadata TEXT,started_ms INTEGER);
            CREATE TABLE segments(session_id TEXT,listen_ms REAL);
            CREATE TABLE track_tags(track_id TEXT,genres TEXT,genres_override INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE auto_genres(track_id TEXT PRIMARY KEY,genres TEXT,source TEXT,checked_ms INTEGER);").unwrap();
        let track = track();
        db.execute(
            "INSERT INTO sessions VALUES ('one',?1,?2,1000)",
            params![track.id, serde_json::to_string(&track).unwrap()],
        )
        .unwrap();
        db.execute("INSERT INTO segments VALUES ('one',10000)", [])
            .unwrap();
        assert!(next(&db).unwrap().is_some());
        save(&db, &track.id, "Alternative", "apple").unwrap();
        assert!(next(&db).unwrap().is_none());
        save(&db, &track.id, "", "apple").unwrap();
        assert!(next(&db).unwrap().is_none());
        save(&db, &track.id, "", "error").unwrap();
        assert!(next(&db).unwrap().is_none());
        db.execute("UPDATE auto_genres SET checked_ms=0", [])
            .unwrap();
        assert!(next(&db).unwrap().is_some());
        db.execute(
            "INSERT INTO track_tags(track_id,genres) VALUES (?1,'')",
            [&track.id],
        )
        .unwrap();
        assert!(next(&db).unwrap().is_some());
        db.execute(
            "UPDATE track_tags SET genres='Rock' WHERE track_id=?1",
            [&track.id],
        )
        .unwrap();
        assert!(next(&db).unwrap().is_none());
        db.execute(
            "UPDATE track_tags SET genres='',genres_override=1 WHERE track_id=?1",
            [&track.id],
        )
        .unwrap();
        assert!(next(&db).unwrap().is_none());
    }
}
