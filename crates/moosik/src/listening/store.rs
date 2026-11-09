use super::*;
use anyhow::{Context as _, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::io::Write;

pub(super) fn open(path: &std::path::Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?;
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(2))?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > 1 {
        bail!("This listening database was created by a newer Moosik. It was left untouched.");
    }
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
        BEGIN;
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY, track_id TEXT NOT NULL, title TEXT NOT NULL,
            metadata TEXT NOT NULL, context TEXT NOT NULL, source TEXT NOT NULL,
            started_ms INTEGER NOT NULL, last_ms INTEGER NOT NULL, ended_ms INTEGER,
            outcome TEXT NOT NULL DEFAULT 'active', duration REAL NOT NULL DEFAULT 0,
            coverage_ms REAL NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS sessions_track ON sessions(track_id,started_ms);
        CREATE INDEX IF NOT EXISTS sessions_start ON sessions(started_ms);
        CREATE TABLE IF NOT EXISTS segments (
            id INTEGER PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id),
            start_ms INTEGER NOT NULL,end_ms INTEGER NOT NULL, position_start REAL,position_end REAL,
            listen_ms REAL NOT NULL,media_ms REAL NOT NULL,paused_ms REAL NOT NULL,buffer_ms REAL NOT NULL,
            muted_ms REAL NOT NULL,unobserved_ms REAL NOT NULL,day TEXT NOT NULL,hour INTEGER NOT NULL,
            weekday INTEGER NOT NULL,offset_seconds INTEGER NOT NULL,volume REAL,speed REAL);
        CREATE INDEX IF NOT EXISTS segments_time ON segments(end_ms,start_ms);
        CREATE INDEX IF NOT EXISTS segments_session ON segments(session_id);
        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY,session_id TEXT NOT NULL REFERENCES sessions(id),
            at_ms INTEGER NOT NULL,kind TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS events_session ON events(session_id,at_ms);
        CREATE TABLE IF NOT EXISTS track_tags (
            track_id TEXT PRIMARY KEY,genres TEXT NOT NULL DEFAULT '',tags TEXT NOT NULL DEFAULT '',
            rating INTEGER NOT NULL DEFAULT 0,updated_ms INTEGER NOT NULL,
            genres_override INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS tag_edits (
            id INTEGER PRIMARY KEY, track_id TEXT NOT NULL, genres TEXT NOT NULL, tags TEXT NOT NULL,
            rating INTEGER NOT NULL, at_ms INTEGER NOT NULL);
        CREATE TRIGGER IF NOT EXISTS tags_insert_history AFTER INSERT ON track_tags BEGIN
            INSERT INTO tag_edits(track_id,genres,tags,rating,at_ms) VALUES (new.track_id,new.genres,new.tags,new.rating,new.updated_ms);
        END;
        PRAGMA user_version=1;
        COMMIT;")?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS auto_genres (
            track_id TEXT PRIMARY KEY, genres TEXT NOT NULL,
            source TEXT NOT NULL, checked_ms INTEGER NOT NULL);",
    )?;
    // Older ledgers have the same track_tags table without this additive column. A final blank
    // genre after an earlier nonblank edit is an identifiable explicit clear. Keep that clear,
    // while a row that has only ever held tags/rating remains eligible for catalog matching.
    let has_genres_override = db
        .prepare("PRAGMA table_info(track_tags)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "genres_override");
    if !has_genres_override {
        db.execute_batch(
            "BEGIN;
             DROP TRIGGER IF EXISTS tags_update_history;
             ALTER TABLE track_tags ADD COLUMN genres_override INTEGER NOT NULL DEFAULT 0;
             UPDATE track_tags SET genres_override=1
             WHERE genres='' AND EXISTS (
                 SELECT 1 FROM tag_edits e
                 WHERE e.track_id=track_tags.track_id AND trim(e.genres)!='');
             COMMIT;",
        )?;
    }
    // The migration's metadata update is not a user edit, so recreate the audit trigger after it.
    db.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS tags_update_history AFTER UPDATE ON track_tags BEGIN
            INSERT INTO tag_edits(track_id,genres,tags,rating,at_ms)
            VALUES (new.track_id,new.genres,new.tags,new.rating,new.updated_ms);
        END;",
    )?;
    // A crash never earns the missing time. Preserve the committed samples and mark the attempt.
    db.execute_batch("BEGIN;
        INSERT INTO events(session_id,at_ms,kind,payload)
            SELECT id,last_ms,'recovered_after_interruption','{}' FROM sessions WHERE ended_ms IS NULL;
        UPDATE sessions SET ended_ms=last_ms,outcome='interrupted' WHERE ended_ms IS NULL;
        COMMIT;")?;
    install_weekly_totals(&db)?;
    Ok(db)
}

/// Backfill once from the local dates recorded at playback. The trigger keeps the
/// summary atomic with new segments, including segments written by an older app.
fn install_weekly_totals(db: &Connection) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    if !tx.table_exists(None, "weekly_listening")? {
        tx.execute_batch(
            "CREATE TABLE weekly_listening (
                monday TEXT PRIMARY KEY, listen_ms REAL NOT NULL
            );
            INSERT INTO weekly_listening
                SELECT date(day, 'weekday 0', '-6 days') AS monday, SUM(listen_ms)
                FROM segments WHERE listen_ms > 0 GROUP BY monday;
            CREATE TRIGGER credit_listening_week AFTER INSERT ON segments
            WHEN NEW.listen_ms > 0 BEGIN
                INSERT INTO weekly_listening(monday, listen_ms)
                    VALUES (date(NEW.day, 'weekday 0', '-6 days'), NEW.listen_ms)
                    ON CONFLICT(monday) DO UPDATE SET listen_ms = listen_ms + excluded.listen_ms;
            END;",
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn peak_week_minutes(db: &Connection) -> Result<u32> {
    Ok(db.query_row(
        "SELECT CAST(COALESCE(MAX(listen_ms), 0) / 60000 AS INTEGER) FROM weekly_listening",
        [],
        |row| row.get(0),
    )?)
}
fn record(db: &mut Connection, records: impl AsRef<[Record]>) -> Result<()> {
    let tx = db.transaction()?;
    for record in records.as_ref() {
        match record {
            Record::Start {
                id,
                at,
                track,
                context,
            } => {
                tx.execute("INSERT INTO sessions(id,track_id,title,metadata,context,source,started_ms,last_ms,duration) VALUES (?1,?2,?3,?4,?5,?6,?7,?7,?8)",
                    params![id,track.id,track.title,serde_json::to_string(track)?,serde_json::to_string(context)?,context.source,at,track.duration])?;
                tx.execute(
                    "INSERT INTO events(session_id,at_ms,kind,payload) VALUES (?1,?2,'start',?3)",
                    params![id, at, serde_json::to_string(context)?],
                )?;
            }
            Record::Event {
                id,
                at,
                kind,
                payload,
            } => {
                tx.execute(
                    "INSERT INTO events(session_id,at_ms,kind,payload) VALUES (?1,?2,?3,?4)",
                    params![id, at, kind, serde_json::to_string(payload)?],
                )?;
            }
            Record::End { id, at, reason } => {
                tx.execute(
                    "UPDATE sessions SET ended_ms=?2,last_ms=?2,outcome=?3 WHERE id=?1",
                    params![id, at, reason],
                )?;
                tx.execute(
                    "INSERT INTO events(session_id,at_ms,kind,payload) VALUES (?1,?2,'end',?3)",
                    params![id, at, serde_json::json!({"reason":reason}).to_string()],
                )?;
            }
            Record::Segment(s) => {
                tx.execute("INSERT INTO segments(session_id,start_ms,end_ms,position_start,position_end,listen_ms,media_ms,paused_ms,buffer_ms,muted_ms,unobserved_ms,day,hour,weekday,offset_seconds,volume,speed) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                    params![s.session,s.start_ms,s.end_ms,s.from,s.to,s.listen_ms,s.media_ms,s.paused_ms,s.buffer_ms,s.muted_ms,s.unobserved_ms,s.day,s.hour,s.weekday,s.offset_seconds,s.volume,s.speed])?;
                tx.execute("UPDATE sessions SET last_ms=?2,coverage_ms=?3,duration=CASE WHEN ?4>0 THEN ?4 ELSE duration END WHERE id=?1",params![s.session,s.end_ms,s.coverage_ms,s.duration])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}
fn clean_list(s: &str) -> String {
    let mut parts: Vec<_> = s
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.chars().take(80).collect::<String>())
        .collect();
    parts.sort_by_key(|s| s.to_lowercase());
    parts.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    parts.truncate(30);
    parts.join(", ")
}
pub(super) fn worker(
    commands: mpsc::Receiver<Command>,
    replies: &mpsc::Sender<Reply>,
) -> Result<()> {
    let path = path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut lock_options = std::fs::OpenOptions::new();
    lock_options
        .read(true)
        .write(true)
        .create(true)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        lock_options.mode(0o600);
    }
    let lock = lock_options.open(path.with_extension("lock"))?;
    lock.try_lock()
        .context("Another Moosik instance is already recording. Close the extra instance.")?;
    let mut db = open(&path)?;
    let _genre_job = genre::Job::start(path.clone(), replies.clone());
    let mut peak = peak_week_minutes(&db)?;
    let collection_ready = match collection::Collection::load(&db) {
        Ok(collection) => {
            let _ = replies.send(Reply::Collection(collection));
            true
        }
        Err(error) => {
            tracing::warn!(%error, "could not load listening keepsakes");
            let _ = replies.send(Reply::CollectionUnavailable);
            false
        }
    };
    // The player polls until this startup reply, including when playback is paused.
    let _ = replies.send(Reply::PeakWeekMinutes(peak));
    while let Ok(command) = commands.recv() {
        let result: Result<()> = match command {
            Command::Records(records) => {
                // Failed writes stop the recorder rather than silently dropping samples and
                // continuing with plausible-looking totals. UI displays the failure persistently.
                record(&mut db, &records).context("Could not save listening data")?;
                if collection_ready {
                    match collection::updates(&db, &records) {
                        Ok((days, songs)) if !days.is_empty() || !songs.is_empty() => {
                            let _ = replies.send(Reply::Credits(days, songs));
                        }
                        Err(e) => tracing::warn!(%e, "could not refresh listening keepsakes"),
                        _ => {}
                    }
                }
                let minutes = peak_week_minutes(&db)?;
                if minutes != peak {
                    peak = minutes;
                    let _ = replies.send(Reply::PeakWeekMinutes(peak));
                }
                Ok(())
            }
            Command::Report(query) => report::build(&db, &query).map(|report| {
                let _ = replies.send(Reply::Report(Box::new(report)));
            }),
            Command::Tags(id, genres, tags, rating, genre_changed) => {
                save_tags(&db, &id, &genres, &tags, rating, genre_changed)
            }
            Command::Export(kind, path, query) => export(&db, kind, &path, &query).map(|()| {
                let _ = replies.send(Reply::Saved(format!("Saved {}", path.display())));
            }),
            Command::Shutdown => {
                db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
                break;
            }
        };
        if let Err(e) = result {
            let _ = replies.send(Reply::Error(format!("Listening operation failed: {e:#}")));
        }
    }
    Ok(())
}

fn save_tags(
    db: &Connection,
    id: &str,
    genres: &str,
    tags: &str,
    rating: i32,
    genre_changed: bool,
) -> Result<()> {
    let previous: Option<(String, i64)> = db
        .query_row(
            "SELECT genres,genres_override FROM track_tags WHERE track_id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    // A catalog match can arrive while the editor is open. Only an edit to its genre field
    // may replace or suppress that match; saving tags/rating preserves the manual row.
    let (genres, genres_override) = if genre_changed {
        (clean_list(genres), 1)
    } else {
        previous.unwrap_or_default()
    };
    db.execute(
        "INSERT INTO track_tags(track_id,genres,tags,rating,updated_ms,genres_override)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(track_id) DO UPDATE SET genres=excluded.genres,tags=excluded.tags,
             rating=excluded.rating,updated_ms=excluded.updated_ms,
             genres_override=excluded.genres_override",
        params![
            id,
            genres,
            clean_list(tags),
            rating.clamp(0, 5),
            now_ms(),
            genres_override
        ],
    )?;
    Ok(())
}

fn export(db: &Connection, kind: Export, path: &std::path::Path, query: &Query) -> Result<()> {
    let live = super::path()?;
    if path == live || (path.exists() && path.canonicalize().ok() == live.canonicalize().ok()) {
        bail!("Choose a different export filename; this is your live listening database.");
    }
    if matches!(kind, Export::Database) {
        // backup includes the live WAL; copying only the main SQLite file would lose recent plays.
        let tmp = path.with_extension(format!("{}.sqlite3.tmp", now_ms()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&tmp)?;
        let result = db
            .backup(rusqlite::MAIN_DB, &tmp, None)
            .map_err(anyhow::Error::from);
        if result.is_ok() {
            std::fs::rename(&tmp, path)?;
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
        return result;
    }
    let report = report::build(db, query)?;
    let tmp = path.with_extension(format!("{}.tmp", now_ms()));
    let result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut out = std::io::BufWriter::new(options.open(&tmp)?);
        match kind {
            Export::Csv => {
                writeln!(
                    out,
                    "track_id,title,artists,album,genres,tags,rating,listening_seconds,plays,attempts,skips,completed,first_play_utc,last_play_utc"
                )?;
                for r in &report.tracks {
                    let cells = vec![
                        r.id.clone(),
                        r.name.clone(),
                        r.subtitle.clone(),
                        r.album.clone(),
                        r.genres.clone(),
                        r.tags.clone(),
                        r.rating.to_string(),
                        format!("{:.3}", r.listen_ms / 1000.0),
                        r.plays.to_string(),
                        r.attempts.to_string(),
                        r.skips.to_string(),
                        r.completed.to_string(),
                        report::date_time(r.first),
                        report::date_time(r.last),
                    ];
                    writeln!(
                        out,
                        "{}",
                        cells.iter().map(|s| csv(s)).collect::<Vec<_>>().join(",")
                    )?;
                }
            }
            Export::Recap => {
                out.write_all(report.recap().as_bytes())?;
            }
            Export::Json => {
                // Full fidelity, all-time raw ledger. Stream rows so years of history do not
                // need to fit in RAM. Credentials and playback URLs were never stored.
                write!(
                    out,
                    "{{\"schema_version\":1,\"exported_at_ms\":{},\"selected_report\":",
                    now_ms()
                )?;
                serde_json::to_writer(&mut out, &report)?;
                for (table, sql) in [
                    (
                        "sessions",
                        "SELECT json_object('id',id,'track_id',track_id,'metadata',json(metadata),'context',json(context),'source',source,'started_ms',started_ms,'last_ms',last_ms,'ended_ms',ended_ms,'outcome',outcome,'duration',duration,'coverage_ms',coverage_ms) FROM sessions ORDER BY started_ms",
                    ),
                    (
                        "segments",
                        "SELECT json_object('id',id,'session_id',session_id,'start_ms',start_ms,'end_ms',end_ms,'position_start',position_start,'position_end',position_end,'listen_ms',listen_ms,'media_ms',media_ms,'paused_ms',paused_ms,'buffer_ms',buffer_ms,'muted_ms',muted_ms,'unobserved_ms',unobserved_ms,'day',day,'hour',hour,'weekday',weekday,'offset_seconds',offset_seconds,'volume',volume,'speed',speed) FROM segments ORDER BY id",
                    ),
                    (
                        "events",
                        "SELECT json_object('id',id,'session_id',session_id,'at_ms',at_ms,'kind',kind,'payload',json(payload)) FROM events ORDER BY id",
                    ),
                    (
                        "tag_edits",
                        "SELECT json_object('id',id,'track_id',track_id,'genres',genres,'tags',tags,'rating',rating,'at_ms',at_ms) FROM tag_edits ORDER BY id",
                    ),
                    (
                        "track_tags",
                        "SELECT json_object('track_id',track_id,'genres',genres,'tags',tags,'rating',rating,'updated_ms',updated_ms,'genres_override',genres_override) FROM track_tags ORDER BY track_id",
                    ),
                    (
                        "auto_genres",
                        "SELECT json_object('track_id',track_id,'genres',genres,'source',source,'checked_ms',checked_ms) FROM auto_genres ORDER BY track_id",
                    ),
                ] {
                    write!(out, ",\"{table}\":[")?;
                    let mut stmt = db.prepare(sql)?;
                    let mut rows = stmt.query([])?;
                    let mut first = true;
                    while let Some(row) = rows.next()? {
                        if !first {
                            write!(out, ",")?;
                        }
                        first = false;
                        let json: String = row.get(0)?;
                        out.write_all(json.as_bytes())?;
                    }
                    write!(out, "]")?;
                }
                write!(out, "}}")?;
            }
            Export::Database => unreachable!(),
        }
        out.flush()?;
        out.get_ref().sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
fn csv(s: &str) -> String {
    // Prevent artist/title metadata from becoming spreadsheet formulas on import.
    let safe = if s.trim_start().starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.into()
    };
    format!("\"{}\"", safe.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "moosik-ledger-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }
    fn metadata() -> Metadata {
        Metadata {
            id: "test".into(),
            title: "Test".into(),
            artists: vec![("Artist".into(), None)],
            album: None,
            album_id: None,
            duration: 100.0,
            explicit: false,
            liked: false,
            video: false,
            artwork: None,
        }
    }
    #[test]
    fn best_week_backfills_once_and_credits_new_playback_across_years() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE segments(day TEXT, listen_ms REAL);
            INSERT INTO segments VALUES ('2025-12-28', 60000), ('2025-12-29', 3600000),
                ('2026-01-04', 14399999), ('2026-01-05', 120000);",
        )
        .unwrap();
        install_weekly_totals(&db).unwrap();
        assert_eq!(peak_week_minutes(&db).unwrap(), 299);
        // A final millisecond in the same local week crosses the five-hour milestone.
        db.execute("INSERT INTO segments VALUES ('2026-01-04', 1)", [])
            .unwrap();
        assert_eq!(peak_week_minutes(&db).unwrap(), 300);
        db.execute_batch("INSERT INTO segments VALUES ('2026-01-05', 0), ('2026-01-11', 60000);")
            .unwrap();
        install_weekly_totals(&db).unwrap();
        assert_eq!(peak_week_minutes(&db).unwrap(), 300);
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM weekly_listening", [], |r| r
                .get::<_, usize>(0))
                .unwrap(),
            3
        );
        db.execute("INSERT INTO segments VALUES ('2026-01-11', 36000000)", [])
            .unwrap();
        assert_eq!(peak_week_minutes(&db).unwrap(), 603);
    }

    #[test]
    fn recovery_preserves_samples_and_marks_interrupted() {
        let dir = temp();
        let p = dir.join("db.sqlite3");
        let mut db = open(&p).unwrap();
        record(
            &mut db,
            vec![
                Record::Start {
                    id: "one".into(),
                    at: 1000,
                    track: metadata(),
                    context: Context::default(),
                },
                Record::Event {
                    id: "one".into(),
                    at: 2000,
                    kind: "pause".into(),
                    payload: serde_json::json!({}),
                },
            ],
        )
        .unwrap();
        drop(db);
        let db = open(&p).unwrap();
        assert_eq!(
            db.query_row("SELECT outcome FROM sessions", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "interrupted"
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_genre_clears_survive_migration_without_suppressing_rating_only_rows() {
        let dir = temp();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite3");
        let old = Connection::open(&path).unwrap();
        old.execute_batch(
            "CREATE TABLE track_tags (
                track_id TEXT PRIMARY KEY,genres TEXT NOT NULL DEFAULT '',
                tags TEXT NOT NULL DEFAULT '',rating INTEGER NOT NULL DEFAULT 0,
                updated_ms INTEGER NOT NULL);
             CREATE TABLE tag_edits (
                id INTEGER PRIMARY KEY,track_id TEXT NOT NULL,genres TEXT NOT NULL,
                tags TEXT NOT NULL,rating INTEGER NOT NULL,at_ms INTEGER NOT NULL);
             CREATE TRIGGER tags_insert_history AFTER INSERT ON track_tags BEGIN
                INSERT INTO tag_edits(track_id,genres,tags,rating,at_ms)
                VALUES (new.track_id,new.genres,new.tags,new.rating,new.updated_ms);
             END;
             CREATE TRIGGER tags_update_history AFTER UPDATE ON track_tags BEGIN
                INSERT INTO tag_edits(track_id,genres,tags,rating,at_ms)
                VALUES (new.track_id,new.genres,new.tags,new.rating,new.updated_ms);
             END;
             INSERT INTO track_tags VALUES ('test','Jazz','Evening',4,100000);
             INSERT INTO track_tags VALUES ('cleared','Soul','',0,100000);
             UPDATE track_tags SET genres='',updated_ms=100001 WHERE track_id='cleared';
             INSERT INTO track_tags VALUES ('rated','','Focus',4,100000);",
        )
        .unwrap();
        drop(old);
        let db = open(&path).unwrap();
        let flags = db
            .prepare("SELECT track_id,genres,genres_override FROM track_tags ORDER BY track_id")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            flags,
            vec![
                ("cleared".into(), "".into(), 1),
                ("rated".into(), "".into(), 0),
                ("test".into(), "Jazz".into(), 0),
            ]
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM tag_edits", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            4,
            "migration must not invent a user edit"
        );
        db.execute("UPDATE track_tags SET rating=5 WHERE track_id='rated'", [])
            .unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM tag_edits", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            5,
            "the audit trigger must remain active"
        );
        drop(db);
        let db = open(&path).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT genres_override FROM track_tags WHERE track_id='cleared'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1,
            "reopening must not reset the migration"
        );
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn exports_preserve_the_ledger_and_live_backup_is_consistent() {
        let dir = temp();
        let mut db = open(&dir.join("db.sqlite3")).unwrap();
        record(
            &mut db,
            vec![
                Record::Start {
                    id: "one".into(),
                    at: 1000,
                    track: metadata(),
                    context: Context {
                        source: "youtube".into(),
                        ..Context::default()
                    },
                },
                Record::Segment(Segment {
                    session: "one".into(),
                    start_ms: 1000,
                    end_ms: 100000,
                    from: 0.0,
                    to: 99.0,
                    listen_ms: 99000.0,
                    media_ms: 99000.0,
                    paused_ms: 0.0,
                    buffer_ms: 0.0,
                    muted_ms: 0.0,
                    unobserved_ms: 0.0,
                    day: "2026-10-26".into(),
                    hour: 12,
                    weekday: 0,
                    offset_seconds: -14400,
                    volume: 70.0,
                    speed: 1.0,
                    coverage_ms: 99000.0,
                    duration: 100.0,
                }),
                Record::End {
                    id: "one".into(),
                    at: 100000,
                    reason: "ended".into(),
                },
            ],
        )
        .unwrap();
        let query = Query {
            from: 0,
            to: 200000,
            source: String::new(),
            label: "Test recap".into(),
        };
        // Editing only tags/rating before a catalog match keeps automatic genre lookup active.
        save_tags(&db, "test", "", "Evening", 5, false).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT genres_override FROM track_tags WHERE track_id='test'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        db.execute("INSERT INTO auto_genres(track_id,genres,source,checked_ms) VALUES ('test','Rock','apple',100000)",[]).unwrap();
        assert_eq!(report::build(&db, &query).unwrap().tracks[0].genres, "Rock");
        // The editor may still hold a blank value from before the match arrived.
        save_tags(&db, "test", "", "Focus", 5, false).unwrap();
        assert_eq!(report::build(&db, &query).unwrap().tracks[0].genres, "Rock");
        // Clearing the visible match is an explicit empty override.
        save_tags(&db, "test", "", "Focus", 5, true).unwrap();
        assert_eq!(report::build(&db, &query).unwrap().tracks[0].genres, "");
        save_tags(&db, "test", "Soul", "Focus", 5, true).unwrap();
        for (kind, name) in [
            (Export::Json, "data.json"),
            (Export::Csv, "songs.csv"),
            (Export::Recap, "recap.md"),
            (Export::Database, "backup.sqlite3"),
        ] {
            export(&db, kind, &dir.join(name), &query).unwrap();
        }
        let data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("data.json")).unwrap()).unwrap();
        assert_eq!(data["selected_report"]["listen_ms"], 99000.0);
        assert_eq!(data["selected_report"]["completed"], 1);
        assert_eq!(data["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(data["segments"].as_array().unwrap().len(), 1);
        assert_eq!(data["tag_edits"].as_array().unwrap().len(), 4);
        assert_eq!(data["auto_genres"][0]["genres"], "Rock");
        assert_eq!(data["track_tags"][0]["genres_override"], 1);
        assert_eq!(data["selected_report"]["genres"][0]["name"], "Soul");
        let backup = Connection::open(dir.join("backup.sqlite3")).unwrap();
        assert_eq!(
            backup
                .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            backup
                .query_row("SELECT sum(listen_ms) FROM segments", [], |r| r
                    .get::<_, f64>(0))
                .unwrap(),
            99000.0
        );
        assert!(
            std::fs::read_to_string(dir.join("recap.md"))
                .unwrap()
                .contains("Test recap")
        );
        assert!(
            std::fs::read_to_string(dir.join("songs.csv"))
                .unwrap()
                .contains("99.000")
        );
        drop(backup);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tags_and_csv_are_safe() {
        assert_eq!(clean_list(" Rock, rock, ,Pop "), "Pop, Rock");
        assert_eq!(csv("=CMD()"), "\"'=CMD()\"");
        assert_eq!(csv("a\"b"), "\"a\"\"b\"");
    }
}
