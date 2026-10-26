use super::*;
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    pub from: i64,
    pub to: i64,
    pub source: String,
    pub label: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Row {
    #[serde(default)]
    pub artwork: Option<String>,
    #[serde(default)]
    pub artist_keys: Vec<String>,
    #[serde(default)]
    pub album_key: String,
    pub id: String,
    pub name: String,
    pub subtitle: String,
    pub album: String,
    pub genres: String,
    pub tags: String,
    pub rating: i32,
    pub listen_ms: f64,
    pub media_ms: f64,
    pub attempts: u64,
    pub plays: u64,
    pub skips: u64,
    pub completed: u64,
    pub first: i64,
    pub last: i64,
    pub coverage: f64,
    pub known_duration: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub listen_ms: f64,
    pub plays: u64,
    pub tracks: usize,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Recent {
    #[serde(default)]
    pub track_id: String,
    #[serde(default)]
    pub artwork: Option<String>,
    pub at: i64,
    pub title: String,
    pub artist: String,
    pub source: String,
    pub outcome: String,
    pub listen_ms: f64,
    pub duration: f64,
    pub coverage: f64,
    pub context: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub query: Query,
    pub generated_ms: i64,
    pub recorded_since: Option<i64>,
    pub listen_ms: f64,
    pub media_ms: f64,
    pub paused_ms: f64,
    pub buffer_ms: f64,
    pub muted_ms: f64,
    pub unobserved_ms: f64,
    pub plays: u64,
    pub attempts: u64,
    pub skips: u64,
    pub completed: u64,
    pub new_tracks: usize,
    pub tracks: Vec<Row>,
    pub artists: Vec<Group>,
    pub albums: Vec<Group>,
    pub genres: Vec<Group>,
    pub sources: Vec<Group>,
    pub days: BTreeMap<String, f64>,
    pub hours: [f64; 24],
    pub weekdays: [f64; 7],
    pub events: BTreeMap<String, u64>,
    pub recent: Vec<Recent>,
    pub longest_streak: usize,
    pub active_days: usize,
    pub previous_ms: f64,
    pub unknown_genre_ms: f64,
    pub total_events: u64,
    pub database_bytes: u64,
}
impl Report {
    pub fn recap(&self) -> String {
        let mut s = format!(
            "# My Moosik recap\n\n{}\n\n- Listening time: {}\n- Qualified plays: {}\n- Different songs: {}\n- Artists: {}\n- New songs first heard in this period: {}\n- Active days: {}\n- Longest day streak: {}\n- Completed listens: {}\n- Skips: {}\n\n## Top songs\n\n",
            self.query.label,
            clock(self.listen_ms),
            self.plays,
            self.tracks.iter().filter(|r| r.listen_ms > 0.0).count(),
            self.artists.len(),
            self.new_tracks,
            self.active_days,
            self.longest_streak,
            self.completed,
            self.skips
        );
        for (i, r) in self
            .tracks
            .iter()
            .filter(|r| r.listen_ms > 0.0)
            .take(20)
            .enumerate()
        {
            s += &format!(
                "{}. {} — {} · {} · {} plays\n",
                i + 1,
                plain(&r.name),
                plain(&r.subtitle),
                clock(r.listen_ms),
                r.plays
            );
        }
        s += "\n## Top artists\n\n";
        for (i, r) in self.artists.iter().take(10).enumerate() {
            s += &format!("{}. {} · {}\n", i + 1, plain(&r.name), clock(r.listen_ms));
        }
        s += "\n## Genres\n\n";
        for r in &self.genres {
            s += &format!("- {}: {}\n", plain(&r.name), clock(r.listen_ms));
        }
        if let Some((hour, _)) = self
            .hours
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            && self.listen_ms > 0.0
        {
            s += &format!("\nPeak listening hour: {hour:02}:00 (local time when played).\n");
        }
        s += "\nRecorded in Moosik on this Mac. Listening time counts observed progress with app volume above zero; it cannot detect system/device mute. A play requires 30 seconds or half the track, whichever is shorter. Completion requires 90% unique track coverage. Collaborators and multiple genre tags each receive full credit, so their totals overlap. Unknown genres remain Unknown. No listening before tracking began is inferred.\n";
        s
    }
}
fn plain(s: &str) -> String {
    s.replace(['\n', '\r', '*', '`', '[', ']', '<', '>'], " ")
}
pub fn clock(ms: f64) -> String {
    let sec = (ms.max(0.0) / 1000.0).round() as u64;
    if sec >= 3600 {
        format!("{}h {:02}m", sec / 3600, sec / 60 % 60)
    } else if sec >= 60 {
        format!("{}m {:02}s", sec / 60, sec % 60)
    } else {
        format!("{sec}s")
    }
}
pub fn date_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}
fn add_group(
    map: &mut HashMap<String, (Group, HashSet<String>)>,
    id: String,
    name: String,
    track: &str,
    time: f64,
    plays: u64,
) {
    if time <= 0.0 {
        return;
    }
    let entry = map.entry(id.clone()).or_insert_with(|| {
        (
            Group {
                id,
                name,
                ..Group::default()
            },
            HashSet::new(),
        )
    });
    entry.0.listen_ms += time;
    entry.0.plays += plays;
    entry.1.insert(track.into());
    entry.0.tracks = entry.1.len();
}
fn groups(map: HashMap<String, (Group, HashSet<String>)>) -> Vec<Group> {
    let mut rows: Vec<_> = map.into_values().map(|v| v.0).collect();
    rows.sort_by(|a, b| {
        b.listen_ms
            .total_cmp(&a.listen_ms)
            .then(a.name.cmp(&b.name))
    });
    rows
}
const FACTOR: &str = "max(0,min(g.end_ms,?2)-max(g.start_ms,?1))*1.0/max(1,g.end_ms-g.start_ms)";
pub(super) fn build(db: &Connection, q: &Query) -> Result<Report> {
    let mut report = Report {
        query: q.clone(),
        generated_ms: now_ms(),
        recorded_since: db.query_row("SELECT min(started_ms) FROM sessions", [], |r| r.get(0))?,
        listen_ms: 0.0,
        media_ms: 0.0,
        paused_ms: 0.0,
        buffer_ms: 0.0,
        muted_ms: 0.0,
        unobserved_ms: 0.0,
        plays: 0,
        attempts: 0,
        skips: 0,
        completed: 0,
        new_tracks: 0,
        tracks: vec![],
        artists: vec![],
        albums: vec![],
        genres: vec![],
        sources: vec![],
        days: BTreeMap::new(),
        hours: [0.0; 24],
        weekdays: [0.0; 7],
        events: BTreeMap::new(),
        recent: vec![],
        longest_streak: 0,
        active_days: 0,
        previous_ms: 0.0,
        unknown_genre_ms: 0.0,
        total_events: db.query_row("SELECT count(*) FROM events", [], |r| r.get(0))?,
        database_bytes: db.query_row(
            "SELECT page_count*page_size FROM pragma_page_count(),pragma_page_size()",
            [],
            |r| r.get(0),
        )?,
    };
    let sql=format!("SELECT s.id,s.metadata,s.context,s.source,s.started_ms,s.last_ms,s.outcome,s.duration,s.coverage_ms,
        coalesce(sum(g.listen_ms*({FACTOR})),0),coalesce(sum(g.media_ms*({FACTOR})),0),
        coalesce(sum(g.paused_ms*({FACTOR})),0),coalesce(sum(g.buffer_ms*({FACTOR})),0),
        coalesce(sum(g.muted_ms*({FACTOR})),0),coalesce(sum(g.unobserved_ms*({FACTOR})),0),
        coalesce(t.genres,''),coalesce(t.tags,''),coalesce(t.rating,0)
        FROM sessions s LEFT JOIN segments g ON g.session_id=s.id AND g.end_ms>?1 AND g.start_ms<?2
        LEFT JOIN track_tags t ON t.track_id=s.track_id
        WHERE s.started_ms<?2 AND s.last_ms>=?1 AND (?3='' OR s.source=?3)
        GROUP BY s.id ORDER BY s.started_ms DESC");
    let mut stmt = db.prepare(&sql)?;
    let mut records = stmt.query(params![q.from, q.to, q.source])?;
    let mut tracks: HashMap<String, Row> = HashMap::new();
    let mut artists = HashMap::new();
    let mut albums = HashMap::new();
    let mut genres = HashMap::new();
    let mut sources = HashMap::new();
    while let Some(r) = records.next()? {
        let meta: Metadata = serde_json::from_str(&r.get::<_, String>(1)?)?;
        let context: String = r.get(2)?;
        let source: String = r.get(3)?;
        let started: i64 = r.get(4)?;
        let last: i64 = r.get(5)?;
        let outcome: String = r.get(6)?;
        let duration: f64 = r.get(7)?;
        let coverage_ms: f64 = r.get(8)?;
        let time: f64 = r.get(9)?;
        let media: f64 = r.get(10)?;
        let genre: String = r.get(15)?;
        let tags: String = r.get(16)?;
        let rating: i32 = r.get(17)?;
        let coverage = if duration > 0.0 {
            (coverage_ms / (duration * 1000.0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let threshold = if duration > 0.0 {
            30_000.0_f64.min(duration * 500.0)
        } else {
            30_000.0
        };
        let play = u64::from(time >= threshold && time > 0.0);
        let completion =
            u64::from(coverage >= 0.9 && outcome != "active" && last >= q.from && last < q.to);
        let skip = u64::from(
            matches!(outcome.as_str(), "replaced" | "skip")
                && coverage < 0.9
                && time > 0.0
                && last >= q.from
                && last < q.to,
        );
        let attempt = u64::from(started >= q.from && started < q.to);
        report.listen_ms += time;
        report.media_ms += media;
        report.paused_ms += r.get::<_, f64>(11)?;
        report.buffer_ms += r.get::<_, f64>(12)?;
        report.muted_ms += r.get::<_, f64>(13)?;
        report.unobserved_ms += r.get::<_, f64>(14)?;
        report.attempts += attempt;
        report.plays += play;
        report.skips += skip;
        report.completed += completion;
        let artist_names = meta
            .artists
            .iter()
            .map(|a| a.0.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let row = tracks.entry(meta.id.clone()).or_insert_with(|| Row {
            id: meta.id.clone(),
            artwork: meta.artwork.clone(),
            artist_keys: meta
                .artists
                .iter()
                .map(|(name, id)| id.clone().unwrap_or_else(|| name.to_lowercase()))
                .collect(),
            album_key: meta.album_id.clone().unwrap_or_else(|| {
                format!(
                    "{}:{}",
                    artist_names,
                    meta.album.as_deref().unwrap_or("Unknown album")
                )
            }),
            name: meta.title.clone(),
            subtitle: artist_names.clone(),
            album: meta.album.clone().unwrap_or_default(),
            genres: genre.clone(),
            tags,
            rating,
            first: started,
            last,
            ..Row::default()
        });
        row.listen_ms += time;
        row.media_ms += media;
        row.attempts += attempt;
        row.plays += play;
        row.skips += skip;
        row.completed += completion;
        row.first = row.first.min(started);
        row.last = row.last.max(last);
        if duration > 0.0 {
            row.coverage += coverage;
            row.known_duration += 1;
        }
        let mut seen = HashSet::new();
        for (name, id) in &meta.artists {
            let key = id.clone().unwrap_or_else(|| name.to_lowercase());
            if seen.insert(key.clone()) {
                add_group(&mut artists, key, name.clone(), &meta.id, time, play);
            }
        }
        add_group(
            &mut albums,
            meta.album_id.clone().unwrap_or_else(|| {
                format!(
                    "{}:{}",
                    artist_names,
                    meta.album.as_deref().unwrap_or("Unknown album")
                )
            }),
            meta.album.clone().unwrap_or_else(|| "Unknown album".into()),
            &meta.id,
            time,
            play,
        );
        let labels = if genre.is_empty() {
            vec!["Unknown"]
        } else {
            genre.split(',').map(str::trim).collect()
        };
        if genre.is_empty() {
            report.unknown_genre_ms += time;
        }
        for label in labels {
            add_group(
                &mut genres,
                label.to_lowercase(),
                label.into(),
                &meta.id,
                time,
                play,
            );
        }
        add_group(
            &mut sources,
            source.clone(),
            source.clone(),
            &meta.id,
            time,
            play,
        );
        if report.recent.len() < 1000 {
            report.recent.push(Recent {
                track_id: meta.id.clone(),
                artwork: meta.artwork.clone(),
                at: started,
                title: meta.title,
                artist: artist_names,
                source,
                outcome,
                listen_ms: time,
                duration,
                coverage,
                context,
            });
        }
    }
    report.tracks = tracks.into_values().collect();
    report.tracks.sort_by(|a, b| {
        b.listen_ms
            .total_cmp(&a.listen_ms)
            .then(a.name.cmp(&b.name))
    });
    report.artists = groups(artists);
    report.albums = groups(albums);
    report.genres = groups(genres);
    report.sources = groups(sources);
    // First-ever listening, not first appearance in a recent date filter.
    report.new_tracks=db.query_row("SELECT count(*) FROM (SELECT s.track_id,min(g.start_ms) first FROM sessions s JOIN segments g ON g.session_id=s.id WHERE g.listen_ms>0 AND (?3='' OR s.source=?3) GROUP BY s.track_id HAVING first>=?1 AND first<?2)",params![q.from,q.to,q.source],|r|r.get(0))?;
    let sql = format!(
        "SELECT g.day,g.hour,g.weekday,sum(g.listen_ms*({FACTOR})) FROM segments g JOIN sessions s ON s.id=g.session_id WHERE g.end_ms>?1 AND g.start_ms<?2 AND (?3='' OR s.source=?3) GROUP BY g.day,g.hour,g.weekday"
    );
    let mut stmt = db.prepare(&sql)?;
    let mut rows = stmt.query(params![q.from, q.to, q.source])?;
    while let Some(r) = rows.next()? {
        let ms: f64 = r.get(3)?;
        if ms <= 0.0 {
            continue;
        }
        *report.days.entry(r.get(0)?).or_default() += ms;
        let h: usize = r.get(1)?;
        let d: usize = r.get(2)?;
        if h < 24 {
            report.hours[h] += ms;
        }
        if d < 7 {
            report.weekdays[d] += ms;
        }
    }
    report.active_days = report.days.len();
    let mut previous = None;
    let mut streak = 0;
    for day in report.days.keys() {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") {
            streak = if previous == date.pred_opt() {
                streak + 1
            } else {
                1
            };
            report.longest_streak = report.longest_streak.max(streak);
            previous = Some(date);
        }
    }
    let mut stmt=db.prepare("SELECT e.kind,count(*) FROM events e JOIN sessions s ON s.id=e.session_id WHERE e.at_ms>=?1 AND e.at_ms<?2 AND (?3='' OR s.source=?3) GROUP BY e.kind")?;
    for row in stmt.query_map(params![q.from, q.to, q.source], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?))
    })? {
        let (k, n) = row?;
        report.events.insert(k, n);
    }
    if q.from > 0 {
        let span = q.to.saturating_sub(q.from);
        let prev_from = q.from.saturating_sub(span).max(0);
        let sql = format!(
            "SELECT coalesce(sum(g.listen_ms*({FACTOR})),0) FROM segments g JOIN sessions s ON s.id=g.session_id WHERE g.end_ms>?1 AND g.start_ms<?2 AND (?3='' OR s.source=?3)"
        );
        report.previous_ms =
            db.query_row(&sql, params![prev_from, q.from, q.source], |r| r.get(0))?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_clips_time_and_collaborators_do_not_double_total() {
        let dir =
            std::env::temp_dir().join(format!("moosik-report-{}-{}", std::process::id(), now_ms()));
        let db = super::super::store::open(&dir.join("db")).unwrap();
        let meta = Metadata {
            id: "song".into(),
            title: "Song".into(),
            artists: vec![
                ("A".into(), Some("a".into())),
                ("B".into(), Some("b".into())),
            ],
            album: Some("Album".into()),
            album_id: None,
            duration: 10.0,
            explicit: false,
            liked: false,
            video: false,
            artwork: None,
        };
        db.execute("INSERT INTO sessions(id,track_id,title,metadata,context,source,started_ms,last_ms,ended_ms,outcome,duration,coverage_ms) VALUES ('s','song','Song',?1,'{}','youtube',1000,11000,11000,'ended',10,10000)",[serde_json::to_string(&meta).unwrap()]).unwrap();
        db.execute_batch("INSERT INTO segments(session_id,start_ms,end_ms,position_start,position_end,listen_ms,media_ms,paused_ms,buffer_ms,muted_ms,unobserved_ms,day,hour,weekday,offset_seconds,volume,speed) VALUES ('s',1000,11000,0,10,10000,10000,0,0,0,0,'2026-10-26',12,0,-14400,70,1)").unwrap();
        let q = Query {
            from: 6000,
            to: 12000,
            source: String::new(),
            label: "Test".into(),
        };
        let report = build(&db, &q).unwrap();
        assert_eq!(report.listen_ms, 5000.0);
        assert_eq!(report.plays, 1);
        assert_eq!(report.completed, 1);
        assert_eq!(report.artists.len(), 2);
        assert!(report.artists.iter().all(|a| a.listen_ms == 5000.0));
        assert_eq!(report.genres[0].name, "Unknown");
        assert_eq!(report.attempts, 0);
        let empty = build(
            &db,
            &Query {
                source: "local".into(),
                ..q
            },
        )
        .unwrap();
        assert_eq!(empty.listen_ms, 0.0);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
