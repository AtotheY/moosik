use super::*;
use anyhow::{Context as _, bail};
use rusqlite::{Connection, params};
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
            rating INTEGER NOT NULL DEFAULT 0,updated_ms INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS tag_edits (
            id INTEGER PRIMARY KEY, track_id TEXT NOT NULL, genres TEXT NOT NULL, tags TEXT NOT NULL,
            rating INTEGER NOT NULL, at_ms INTEGER NOT NULL);
        CREATE TRIGGER IF NOT EXISTS tags_insert_history AFTER INSERT ON track_tags BEGIN
            INSERT INTO tag_edits(track_id,genres,tags,rating,at_ms) VALUES (new.track_id,new.genres,new.tags,new.rating,new.updated_ms);
        END;
        CREATE TRIGGER IF NOT EXISTS tags_update_history AFTER UPDATE ON track_tags BEGIN
            INSERT INTO tag_edits(track_id,genres,tags,rating,at_ms) VALUES (new.track_id,new.genres,new.tags,new.rating,new.updated_ms);
        END;
        PRAGMA user_version=1;
        COMMIT;")?;
    // A crash never earns the missing time. Preserve the committed samples and mark the attempt.
    db.execute_batch("BEGIN;
        INSERT INTO events(session_id,at_ms,kind,payload)
            SELECT id,last_ms,'recovered_after_interruption','{}' FROM sessions WHERE ended_ms IS NULL;
        UPDATE sessions SET ended_ms=last_ms,outcome='interrupted' WHERE ended_ms IS NULL;
        COMMIT;")?;
    Ok(db)
}
fn record(db: &mut Connection, records: Vec<Record>) -> Result<()> {
    let tx = db.transaction()?;
    for record in records {
        match record {
            Record::Start {
                id,
                at,
                track,
                context,
            } => {
                tx.execute("INSERT INTO sessions(id,track_id,title,metadata,context,source,started_ms,last_ms,duration) VALUES (?1,?2,?3,?4,?5,?6,?7,?7,?8)",
                    params![id,track.id,track.title,serde_json::to_string(&track)?,serde_json::to_string(&context)?,context.source,at,track.duration])?;
                tx.execute(
                    "INSERT INTO events(session_id,at_ms,kind,payload) VALUES (?1,?2,'start',?3)",
                    params![id, at, serde_json::to_string(&context)?],
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
                    params![id, at, kind, serde_json::to_string(&payload)?],
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
    while let Ok(command) = commands.recv() {
        let result:Result<()> = match command {
            Command::Records(records) => {
                // Failed writes stop the recorder rather than silently dropping samples and
                // continuing with plausible-looking totals. UI displays the failure persistently.
                record(&mut db,records).context("Could not save listening data")?; Ok(())
            }
            Command::Report(query) => report::build(&db,&query).map(|report| { let _=replies.send(Reply::Report(Box::new(report))); }),
            Command::Tags(id,genres,tags,rating) => db.execute("INSERT INTO track_tags(track_id,genres,tags,rating,updated_ms) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(track_id) DO UPDATE SET genres=excluded.genres,tags=excluded.tags,rating=excluded.rating,updated_ms=excluded.updated_ms",
                params![id,clean_list(&genres),clean_list(&tags),rating.clamp(0,5),now_ms()]).map(|_|()).map_err(Into::into),
            Command::Export(kind,path,query) => export(&db,kind,&path,&query).map(|()| { let _=replies.send(Reply::Saved(format!("Saved {}",path.display()))); }),
            Command::Shutdown => { db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?; break; }
        };
        if let Err(e) = result {
            let _ = replies.send(Reply::Error(format!("Listening operation failed: {e:#}")));
        }
    }
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
                        "SELECT json_object('track_id',track_id,'genres',genres,'tags',tags,'rating',rating,'updated_ms',updated_ms) FROM track_tags ORDER BY track_id",
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
        db.execute("INSERT INTO track_tags(track_id,genres,tags,rating,updated_ms) VALUES ('test','Jazz','Evening',5,100000)",[]).unwrap();
        db.execute(
            "UPDATE track_tags SET genres='Soul',updated_ms=100001 WHERE track_id='test'",
            [],
        )
        .unwrap();
        let query = Query {
            from: 0,
            to: 200000,
            source: String::new(),
            label: "Test recap".into(),
        };
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
        assert_eq!(data["tag_edits"].as_array().unwrap().len(), 2);
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
