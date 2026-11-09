//! Lifetime keepsakes derived from credited listening, independent of the stats filters.
use super::{Metadata, Record, report::qualified_play};
use anyhow::Result;
use rusqlite::{Connection, Row};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default)]
pub struct Collection {
    pub revision: u64,
    pub sleeves: HashMap<String, Sleeve>,
    pub passes: BTreeMap<String, ArtistPass>,
}

#[derive(Default)]
pub struct Sleeve {
    pub completed: u64,
    pub first_day: String,
    pub title: String,
    pub artwork: Option<std::sync::Arc<str>>,
    pub artist: String,
    pub listen_ms: f64,
}

#[derive(Default)]
pub struct ArtistPass {
    pub name: String,
    pub listen_ms: f64,
    songs: HashSet<String>,
    days: HashSet<String>,
    pub stamps: Vec<Stamp>,
}
pub struct Stamp {
    pub day: String,
    pub song: String,
}

pub const TIERS: [&str; 4] = ["Admit One", "Regular", "Backstage", "All Access"];
impl ArtistPass {
    pub fn songs(&self) -> usize {
        self.songs.len()
    }

    pub fn days(&self) -> usize {
        self.days.len()
    }

    pub fn next_days(&self) -> Option<usize> {
        match self.stamps.len() {
            1 => Some(15),
            2 => Some(30),
            3 => Some(60),
            _ => None,
        }
    }

    fn credit(&mut self, track: &Metadata, day: &str) {
        self.songs.insert(track.id.clone());
        self.days.insert(day.to_owned());
        let unlock = match self.next_days() {
            Some(target) => self.days.len() >= target,
            None => self.stamps.is_empty() && (self.songs.len() >= 10 || self.days.len() >= 5),
        };
        if unlock {
            self.stamps.push(Stamp {
                day: day.to_owned(),
                song: track.title.clone(),
            });
        }
    }
}

pub fn artist_key(name: &str, id: Option<&str>) -> String {
    id.filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| name.trim().to_lowercase())
}

pub(super) struct Hearing {
    track: Metadata,
    day: String,
    qualified: bool,
    listen_ms: f64,
}
impl Hearing {
    fn read(row: &Row<'_>) -> Result<Self> {
        let track = serde_json::from_str(&row.get::<_, String>(0)?)?;
        Ok(Self {
            track,
            day: row.get(1)?,
            listen_ms: row.get(2)?,
            qualified: qualified_play(row.get(2)?, row.get(3)?) > 0,
        })
    }
}

impl Collection {
    pub(super) fn hear(&mut self, hearing: Hearing) {
        let Hearing {
            track,
            day,
            qualified,
            listen_ms,
        } = hearing;
        self.revision += 1;
        let sleeve = self.sleeves.entry(track.id.clone()).or_default();
        sleeve.listen_ms += listen_ms;
        sleeve.title.clone_from(&track.title);
        if let Some(url) = track.artwork.as_deref()
            && sleeve.artwork.as_deref() != Some(url)
        {
            sleeve.artwork = Some(url.into());
        }
        sleeve.artist = track
            .artists
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if sleeve.first_day.is_empty() || day < sleeve.first_day {
            sleeve.first_day.clone_from(&day);
        }
        let mut credited = HashSet::new();
        for (name, id) in &track.artists {
            if name.trim().is_empty() {
                continue;
            }
            let key = artist_key(name, id.as_deref());
            if !credited.insert(key.clone()) {
                continue;
            }
            let pass = self.passes.entry(key).or_default();
            pass.name.clone_from(name);
            pass.listen_ms += listen_ms;
            if qualified {
                pass.credit(&track, &day);
            }
        }
    }

    pub(super) fn load(db: &Connection) -> Result<Self> {
        // Older versions keep writing segments, so maintain this compact index in the
        // same transaction as each segment. Restarts then read one row per session/day.
        let tx = db.unchecked_transaction()?;
        if !tx.table_exists(None, "heard_days")? {
            tx.execute_batch("CREATE TABLE heard_days (
                session_id TEXT NOT NULL, day TEXT NOT NULL, first_ms INTEGER NOT NULL,
                listen_ms REAL NOT NULL, PRIMARY KEY(session_id, day));
                INSERT INTO heard_days SELECT session_id, day, MIN(start_ms), SUM(listen_ms)
                    FROM segments WHERE listen_ms > 0 GROUP BY session_id, day;
                CREATE TRIGGER credit_heard_day AFTER INSERT ON segments WHEN NEW.listen_ms > 0 BEGIN
                    INSERT INTO heard_days VALUES (NEW.session_id, NEW.day, NEW.start_ms, NEW.listen_ms)
                    ON CONFLICT(session_id,day) DO UPDATE SET listen_ms=listen_ms+excluded.listen_ms,
                        first_ms=MIN(first_ms,excluded.first_ms);
                END;")?;
        }
        tx.commit()?;
        let mut collection = Self::default();
        let mut stmt = db.prepare(&format!("{HEARINGS} ORDER BY h.first_ms,s.id,h.day"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            collection.hear(Hearing::read(row)?);
        }
        let mut stmt = db.prepare(&format!(
            "SELECT track_id, COUNT(*) FROM sessions WHERE {COMPLETED} GROUP BY track_id"
        ))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            if let Some(sleeve) = collection.sleeves.get_mut(&row.get::<_, String>(0)?) {
                sleeve.completed = row.get(1)?;
            }
        }
        Ok(collection)
    }
}

const HEARINGS: &str = "SELECT s.metadata,h.day,h.listen_ms,s.duration FROM heard_days h
    JOIN sessions s ON s.id=h.session_id";
const COMPLETED: &str = "ended_ms IS NOT NULL AND duration > 0 AND coverage_ms >= duration * 900.0";

/// Read only committed rows touched by this batch. Per-day credit is idempotent;
/// completion is delivered once with the session's final End record.
pub(super) fn updates(db: &Connection, records: &[Record]) -> Result<(Vec<Hearing>, Vec<String>)> {
    let mut hearings = Vec::new();
    let mut completed = Vec::new();
    // The query returns cumulative daily credit for milestone qualification. Add
    // only this committed batch's new time to the lifetime listening totals.
    let mut deltas = HashMap::new();
    for record in records {
        if let Record::Segment(s) = record
            && s.listen_ms > 0.
        {
            *deltas.entry((&s.session, &s.day)).or_insert(0.) += s.listen_ms;
        }
    }
    for record in records {
        match record {
            Record::Segment(s) if s.listen_ms > 0.0 => {
                let Some(delta) = deltas.remove(&(&s.session, &s.day)) else {
                    continue;
                };
                let mut stmt =
                    db.prepare_cached(&format!("{HEARINGS} WHERE h.session_id=?1 AND h.day=?2"))?;
                let mut rows = stmt.query((&s.session, &s.day))?;
                if let Some(row) = rows.next()? {
                    let mut hearing = Hearing::read(row)?;
                    hearing.listen_ms = delta;
                    hearings.push(hearing);
                }
            }
            Record::End { id, .. } => {
                let mut stmt = db.prepare_cached(&format!(
                    "SELECT track_id FROM sessions WHERE id=?1 AND {COMPLETED}"
                ))?;
                let mut rows = stmt.query([id])?;
                if let Some(row) = rows.next()? {
                    completed.push(row.get(0)?);
                }
            }
            _ => {}
        }
    }
    Ok((hearings, completed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(song: usize, day: chrono::NaiveDate, qualified: bool) -> Hearing {
        Hearing {
            track: Metadata {
                id: format!("track-{song}"),
                title: format!("Song {song}"),
                artists: vec![("Artist".into(), Some("id".into()))],
                ..Default::default()
            },
            day: day.to_string(),
            listen_ms: 30000.,
            qualified,
        }
    }

    #[test]
    fn nonconsecutive_days_and_unique_songs_stamp_only_once() {
        let mut c = Collection::default();
        let start = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        c.hear(heard(0, start, false));
        assert_eq!(c.passes["id"].days(), 0);
        for i in 0..9 {
            c.hear(heard(i, start, true));
        }
        assert!(c.passes["id"].stamps.is_empty());
        c.hear(heard(9, start, true));
        assert_eq!(c.passes["id"].stamps[0].song, "Song 9");
        for i in 1..60 {
            let day = start + chrono::Days::new(i * 2);
            for _ in 0..3 {
                c.hear(heard(9, day, true));
            }
        }
        let pass = &c.passes["id"];
        assert_eq!(pass.days(), 60);
        let dates: Vec<_> = pass.stamps.iter().map(|s| s.day.clone()).collect();
        assert_eq!(
            dates,
            [0, 14, 29, 59].map(|d| (start + chrono::Days::new(d * 2)).to_string())
        );
        let mut single = Collection::default();
        for i in 0..5 {
            single.hear(heard(0, start + chrono::Days::new(i * 6), true));
        }
        assert_eq!(single.passes["id"].stamps.len(), 1);
    }

    #[test]
    fn historical_and_live_sessions_use_the_same_days_and_full_listens() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE sessions(id TEXT PRIMARY KEY,track_id TEXT,metadata TEXT,duration REAL,coverage_ms REAL,ended_ms INTEGER);
            CREATE TABLE segments(session_id TEXT,day TEXT,start_ms INTEGER,listen_ms REAL);").unwrap();
        let meta = serde_json::to_string(&Metadata {
            id: "song".into(),
            title: "Song".into(),
            artists: vec![("Artist".into(), None)],
            ..Default::default()
        })
        .unwrap();
        for i in 1..=5 {
            db.execute(
                "INSERT INTO sessions VALUES (?1,'song',?2,100,90000,1)",
                (i.to_string(), &meta),
            )
            .unwrap();
            db.execute(
                "INSERT INTO segments VALUES (?1,?2,?3,30000)",
                (i.to_string(), format!("2026-03-0{i}"), i),
            )
            .unwrap();
        }
        let mut initial = Collection::load(&db).unwrap();
        assert_eq!(initial.sleeves["song"].completed, 5);
        assert_eq!(initial.sleeves["song"].first_day, "2026-03-01");
        assert_eq!(initial.passes["artist"].stamps[0].day, "2026-03-05");
        db.execute(
            "INSERT INTO sessions VALUES ('new','song',?1,100,89999,2)",
            [&meta],
        )
        .unwrap();
        db.execute_batch("INSERT INTO segments VALUES ('new','2026-03-09',6,31000);")
            .unwrap();
        let end = Record::End {
            id: "new".into(),
            at: 2,
            reason: "ended".into(),
        };
        assert!(
            updates(&db, std::slice::from_ref(&end))
                .unwrap()
                .1
                .is_empty()
        );
        db.execute_batch("UPDATE sessions SET coverage_ms=90000 WHERE id='new'")
            .unwrap();
        assert_eq!(updates(&db, &[end]).unwrap().1, ["song"]);
        // A batch can contain several samples for a previously credited day.
        // Re-reading cumulative credit must not count the old samples again.
        let day = "2026-03-05";
        let sample = Record::Segment(super::super::Segment {
            session: "5".into(),
            day: day.into(),
            start_ms: 7,
            end_ms: 8,
            from: 0.,
            to: 0.,
            listen_ms: 500.,
            media_ms: 500.,
            paused_ms: 0.,
            buffer_ms: 0.,
            muted_ms: 0.,
            unobserved_ms: 0.,
            hour: 12,
            weekday: 0,
            offset_seconds: 0,
            volume: 1.,
            speed: 1.,
            coverage_ms: 0.,
            duration: 100.,
        });
        for _ in 0..2 {
            db.execute("INSERT INTO segments VALUES ('5',?1,7,500)", [day])
                .unwrap();
        }
        let (hearings, _) = updates(&db, &[sample.clone(), sample]).unwrap();
        assert_eq!(hearings.len(), 1);
        for h in hearings {
            initial.hear(h);
        }
        assert_eq!(initial.passes["artist"].listen_ms, 151000.);
        let restored = Collection::load(&db).unwrap();
        assert_eq!(restored.sleeves["song"].completed, 6);
        assert_eq!(restored.sleeves["song"].listen_ms, 182000.);
        assert_eq!(restored.passes["artist"].days(), 6);
    }
}
