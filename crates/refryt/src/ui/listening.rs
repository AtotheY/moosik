//! A private listening observatory: raw history, iTunes-style song columns and custom recaps.
use crate::{
    app::App,
    listening::{self, Export, Group, Query, Report},
    theme,
};
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use egui::{RichText, vec2};

#[derive(Default, PartialEq, Clone, Copy)]
enum Tab {
    #[default]
    Overview,
    Songs,
    Artists,
    Albums,
    Genres,
    History,
    Recap,
}
#[derive(Default, PartialEq, Clone, Copy)]
enum Period {
    Today,
    Week,
    Month,
    CalendarMonth,
    Year,
    #[default]
    All,
    Custom,
}
#[derive(Default)]
pub struct View {
    tab: Tab,
    period: Period,
    from: String,
    to: String,
    source: String,
    search: String,
    sort: usize,
    selected: Option<String>,
    genres: String,
    tags: String,
    rating: i32,
}
fn midnight(date: NaiveDate) -> Option<i64> {
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .earliest()
        .map(|v| v.timestamp_millis())
}
impl View {
    fn query(&self) -> Result<Query, String> {
        let today = Local::now().date_naive();
        let tomorrow = today.succ_opt().unwrap_or(today);
        let (from, to, label) = match self.period {
            Period::Today => (Some(today), tomorrow, "Today".into()),
            Period::Week => (
                today.checked_sub_days(chrono::Days::new(6)),
                tomorrow,
                "Last 7 days".into(),
            ),
            Period::Month => (
                today.checked_sub_days(chrono::Days::new(29)),
                tomorrow,
                "Last 30 days".into(),
            ),
            Period::CalendarMonth => (today.with_day(1), tomorrow, "This month".into()),
            Period::Year => (
                NaiveDate::from_ymd_opt(today.year(), 1, 1),
                tomorrow,
                format!("{} so far", today.year()),
            ),
            Period::All => (None, tomorrow, "All time".into()),
            Period::Custom => {
                let start = NaiveDate::parse_from_str(&self.from, "%Y-%m-%d")
                    .map_err(|_| "Enter a start date as YYYY-MM-DD")?;
                let end = NaiveDate::parse_from_str(&self.to, "%Y-%m-%d")
                    .map_err(|_| "Enter an end date as YYYY-MM-DD")?;
                if start > end {
                    return Err("Start date must be on or before end date".into());
                }
                (
                    Some(start),
                    end.succ_opt().ok_or("End date is out of range")?,
                    format!("{} to {}", self.from, self.to),
                )
            }
        };
        Ok(Query {
            from: from
                .map(midnight)
                .transpose_option()
                .ok_or("Start date is not valid in your timezone")?
                .unwrap_or(0),
            to: midnight(to).ok_or("End date is not valid in your timezone")?,
            source: self.source.clone(),
            label,
        })
    }
}
trait TransposeOption<T> {
    fn transpose_option(self) -> Option<Option<T>>;
}
impl<T> TransposeOption<T> for Option<Option<T>> {
    fn transpose_option(self) -> Option<Option<T>> {
        match self {
            None => Some(None),
            Some(v) => v.map(Some),
        }
    }
}

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    app.listening.poll();
    let p = app.palette;
    crate::ui::page_scroll(app,"listening-stats").show(ui,|ui| {
        crate::ui::page_title(ui,&p,"Your listening, in detail.");
        ui.label(RichText::new("A living record of your music. Private, on this Mac, and yours to export.").color(p.secondary).size(14.0));
        ui.add_space(18.0);
        let mut refresh=false;
        ui.horizontal_wrapped(|ui| {
            for (period,label) in [(Period::Today,"Today"),(Period::Week,"7 days"),(Period::Month,"30 days"),(Period::CalendarMonth,"This month"),(Period::Year,"This year"),(Period::All,"All time"),(Period::Custom,"Custom")] {
                if ui.selectable_value(&mut app.listening_view.period,period,label).changed(){refresh=true;}
            }
            ui.add_space(12.0);
            egui::ComboBox::from_id_salt("listening-source").selected_text(if app.listening_view.source.is_empty(){"All sources"}else{&app.listening_view.source}).show_ui(ui,|ui|{
                for (id,label) in [("","All sources"),("youtube","YouTube"),("downloaded","Downloaded"),("local","Local files")] {
                    if ui.selectable_value(&mut app.listening_view.source,id.into(),label).changed(){refresh=true;}
                }
            });
            if ui.button("Refresh").clicked(){refresh=true;}
        });
        if app.listening_view.period==Period::Custom {
            if app.listening_view.from.is_empty(){app.listening_view.from=Local::now().date_naive().with_day(1).unwrap().to_string();app.listening_view.to=Local::now().date_naive().to_string();}
            ui.horizontal(|ui|{
                ui.label("From");ui.add(egui::TextEdit::singleline(&mut app.listening_view.from).desired_width(110.0));
                ui.label("through");ui.add(egui::TextEdit::singleline(&mut app.listening_view.to).desired_width(110.0));
                ui.label(RichText::new("YYYY-MM-DD · inclusive dates").color(p.secondary));
            });
        }
        let query=match app.listening_view.query(){Ok(q)=>q,Err(e)=>{ui.colored_label(p.warning,e);return;}};
        app.listening.request(query.clone(),refresh);
        if let Some(error)=&app.listening.error {ui.colored_label(p.danger,error);}
        if let Some(notice)=&app.listening.notice {ui.colored_label(p.accent_text(),notice);}
        ui.add_space(16.0);
        ui.horizontal_wrapped(|ui|{
            for(tab,label)in[(Tab::Overview,"Overview"),(Tab::Songs,"Songs"),(Tab::Artists,"Artists"),(Tab::Albums,"Albums"),(Tab::Genres,"Genres"),(Tab::History,"History"),(Tab::Recap,"My recap")]{ui.selectable_value(&mut app.listening_view.tab,tab,label);}
        });
        ui.separator();ui.add_space(12.0);
        let Some(report)=app.listening.report.take() else {ui.spinner();ui.label("Opening your listening ledger…");return;};
        if report.query!=query {
            ui.spinner();ui.label("Loading this date range…");
        } else {
            if report.attempts==0 && report.listen_ms==0.0 {
                egui::Frame::new().fill(p.surface).corner_radius(14.0).inner_margin(20.0).show(ui,|ui|{
                    ui.label(RichText::new("Your next song starts the story.").size(23.0).strong());
                    ui.label("Play something in Moosik and this page will fill itself in. Existing YouTube history doesn’t contain reliable listening durations, so we never invent them.");
                });ui.add_space(18.0);
            }
            match app.listening_view.tab {
                Tab::Overview=>overview(ui,&p,&report),
                Tab::Songs=>songs(app,ui,&report),
                Tab::Artists=>ranking(ui,&p,"Artists",&report.artists,"Every credited collaborator receives the track’s full time; artist totals overlap."),
                Tab::Albums=>ranking(ui,&p,"Albums",&report.albums,"Album names and IDs are captured when the track is played."),
                Tab::Genres=>{
                    ui.label(format!("{} of your listening is still unclassified. Select a song in Songs to add genres, mood tags, and a 1–5 star rating.",listening::report::clock(report.unknown_genre_ms)));
                    ranking(ui,&p,"Genres",&report.genres,"Your genre tags apply to past and future listens. Multi-genre tracks credit each genre; totals overlap.");
                }
                Tab::History=>history(ui,&p,&report),
                Tab::Recap=>recap(ui,&p,&report),
            }
            ui.add_space(24.0);
            ui.horizontal_wrapped(|ui|{
                ui.label(RichText::new("Take your data with you").strong());
                for(kind,label,extension)in[(Export::Csv,"Song stats CSV","csv"),(Export::Recap,"My recap","md"),(Export::Json,"Everything JSON","json"),(Export::Database,"Database backup","sqlite3")] {
                    if ui.button(label).clicked() && let Some(path)=rfd::FileDialog::new().set_file_name(format!("moosik-{}.{extension}",Local::now().format("%Y-%m-%d"))).add_filter(label,&[extension]).save_file(){
                        app.listening.export(kind,path,query.clone());
                    }
                }
            });
            ui.label(RichText::new("CSV and recap use the date/source filters. JSON and database backup retain the entire raw history.").small().color(p.secondary));
            ui.add_space(12.0);
            ui.collapsing("How your stats work",|ui|{
                ui.label("Listening time counts observed playback progress, excluding pauses, buffering, app mute, and seeks. It cannot detect system volume, disconnected headphones, or whether you were paying attention.");
                ui.label("A qualified play = 30 seconds or half the track, whichever is shorter. A completed listen = at least 90% unique coverage in an ended session. Replaying a section doesn’t inflate coverage. A skip = leaving a partly heard track for another one.");
                ui.label("Times are clipped to your selected range. Plays are qualified by time inside that range; completions/skips are counted when the session ends in it. Coverage is for the whole session. Song first/last dates refer to the sessions touching this range.");
                ui.label("Samples are taken about once a second and committed every five seconds and at track changes. A crash can lose the last uncommitted seconds; unobserved gaps are never credited. Hour/day patterns preserve the local timezone at playback.");
                ui.label("Recording covers this Moosik installation only, including while minimized. Data stays in your Mac user account; it isn’t uploaded or separately encrypted. No automatic history deletion. Unknown metadata remains Unknown.");
                if let Some(at)=report.recorded_since {ui.label(format!("First recording: {}",local_date(at)));}
                ui.label(format!("{} saved events · {:.1} MB database",report.total_events,report.database_bytes as f64/1_000_000.0));
            });
        }
        app.listening.report=Some(report);
    });
    editor(app, ui.ctx());
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs(1));
}
fn cards(ui: &mut egui::Ui, p: &theme::Palette, items: &[(&str, String)]) {
    let n = if ui.available_width() < 620.0 { 2 } else { 4 };
    for row in items.chunks(n) {
        ui.columns(n, |cols| {
            for (i, (label, value)) in row.iter().enumerate() {
                egui::Frame::new()
                    .fill(p.surface)
                    .corner_radius(12.0)
                    .inner_margin(16.0)
                    .show(&mut cols[i], |ui| {
                        ui.set_min_width((ui.available_width() - 2.0).max(0.0));
                        ui.label(RichText::new(*label).size(12.0).color(p.secondary));
                        ui.add_space(6.0);
                        ui.label(RichText::new(value).size(26.0).strong().color(p.text));
                    });
            }
        });
        ui.add_space(10.0);
    }
}
fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(text).size(19.0).strong());
    ui.add_space(8.0);
}
fn overview(ui: &mut egui::Ui, p: &theme::Palette, r: &Report) {
    cards(
        ui,
        p,
        &[
            ("LISTENING TIME", listening::report::clock(r.listen_ms)),
            ("QUALIFIED PLAYS", r.plays.to_string()),
            (
                "DIFFERENT SONGS",
                r.tracks
                    .iter()
                    .filter(|t| t.listen_ms > 0.0)
                    .count()
                    .to_string(),
            ),
            ("ARTISTS", r.artists.len().to_string()),
            ("ACTIVE DAYS", r.active_days.to_string()),
            ("LONGEST STREAK", format!("{} days", r.longest_streak)),
            ("NEW SONGS", r.new_tracks.to_string()),
            ("COMPLETED LISTENS", r.completed.to_string()),
        ],
    );
    if r.previous_ms > 0.0 {
        ui.label(
            RichText::new(format!(
                "{:+.0}% listening time versus the previous equal-length window",
                (r.listen_ms / r.previous_ms - 1.0) * 100.0
            ))
            .color(p.accent_text()),
        );
    }
    heatmap(ui, p, r);
    ui.columns(2, |cols| {
        heading(&mut cols[0], "Your most-played songs");
        for (i, row) in r
            .tracks
            .iter()
            .filter(|r| r.listen_ms > 0.0)
            .take(5)
            .enumerate()
        {
            bar(
                &mut cols[0],
                p,
                &format!("{}. {}", i + 1, row.name),
                &format!(
                    "{} · {} plays",
                    listening::report::clock(row.listen_ms),
                    row.plays
                ),
                row.listen_ms,
                r.tracks.first().map_or(1.0, |t| t.listen_ms),
            );
        }
        heading(&mut cols[1], "Your most-played artists");
        for (i, row) in r.artists.iter().take(5).enumerate() {
            bar(
                &mut cols[1],
                p,
                &format!("{}. {}", i + 1, row.name),
                &listening::report::clock(row.listen_ms),
                row.listen_ms,
                r.artists.first().map_or(1.0, |t| t.listen_ms),
            );
        }
    });
    heading(ui, "When you press play");
    histogram(
        ui,
        p,
        &r.hours
            .iter()
            .enumerate()
            .map(|(i, v)| (format!("{i:02}"), *v))
            .collect::<Vec<_>>(),
        95.0,
    );
    ui.label(
        RichText::new("Hour of day · local time when listened")
            .small()
            .color(p.secondary),
    );
    histogram(
        ui,
        p,
        &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
            .iter()
            .zip(r.weekdays)
            .map(|(d, v)| (d.to_string(), v))
            .collect::<Vec<_>>(),
        70.0,
    );
    heading(ui, "The details other recaps leave out");
    cards(
        ui,
        p,
        &[
            ("SKIPS", r.skips.to_string()),
            (
                "SEEK EVENTS",
                r.events.get("seek").unwrap_or(&0).to_string(),
            ),
            ("PAUSED", listening::report::clock(r.paused_ms)),
            ("BUFFERING", listening::report::clock(r.buffer_ms)),
            ("APP MUTED", listening::report::clock(r.muted_ms)),
            ("PLAY ATTEMPTS", r.attempts.to_string()),
            (
                "AVERAGE / ACTIVE DAY",
                listening::report::clock(r.listen_ms / r.active_days.max(1) as f64),
            ),
            ("UNOBSERVED TIME", listening::report::clock(r.unobserved_ms)),
        ],
    );
    heading(ui, "Playback sources");
    for row in &r.sources {
        bar(
            ui,
            p,
            &row.name,
            &listening::report::clock(row.listen_ms),
            row.listen_ms,
            r.listen_ms,
        );
    }
}
fn bar(ui: &mut egui::Ui, p: &theme::Palette, label: &str, value: &str, amount: f64, max: f64) {
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(RichText::new(label).size(13.0)).truncate());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).small().color(p.secondary));
        });
    });
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, p.surface);
    let width = rect.width() * (amount / max.max(1.0)).clamp(0.0, 1.0) as f32;
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, vec2(width, 4.0)),
        2.0,
        p.accent,
    );
    ui.add_space(12.0);
}
fn histogram(ui: &mut egui::Ui, p: &theme::Palette, values: &[(String, f64)], height: f32) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), height + 22.0),
        egui::Sense::hover(),
    );
    let max = values.iter().map(|v| v.1).fold(1.0_f64, f64::max);
    let step = rect.width() / values.len().max(1) as f32;
    for (i, (label, value)) in values.iter().enumerate() {
        let x = rect.left() + i as f32 * step;
        let h = (*value / max) as f32 * height;
        let b = egui::Rect::from_min_max(
            egui::pos2(x + 2.0, rect.top() + height - h.max(2.0)),
            egui::pos2(x + step - 2.0, rect.top() + height),
        );
        ui.painter()
            .rect_filled(b, 3.0, if *value > 0.0 { p.accent } else { p.surface });
        ui.painter().text(
            egui::pos2(x + step / 2.0, rect.bottom() - 8.0),
            egui::Align2::CENTER_CENTER,
            label,
            theme::regular(10.0),
            p.secondary,
        );
        ui.interact(
            b,
            ui.id().with(("hist", label, rect.top().to_bits())),
            egui::Sense::hover(),
        )
        .on_hover_text(format!("{label}: {}", listening::report::clock(*value)));
    }
}
fn heatmap(ui: &mut egui::Ui, p: &theme::Palette, r: &Report) {
    heading(ui, "Your listening rhythm");
    let end = chrono::DateTime::from_timestamp_millis(r.query.to - 1)
        .map(|d| d.with_timezone(&Local).date_naive())
        .unwrap_or_else(|| Local::now().date_naive());
    let end = end.min(Local::now().date_naive());
    let start = end - chrono::Days::new(90);
    let max = r.days.values().copied().fold(1.0_f64, f64::max);
    let cell = ((ui.available_width() / 13.0) - 4.0).clamp(8.0, 19.0);
    let (rect, _) = ui.allocate_exact_size(
        vec2(13.0 * (cell + 4.0), 7.0 * (cell + 4.0)),
        egui::Sense::hover(),
    );
    for i in 0..91 {
        let date = start + chrono::Days::new(i);
        let ms = r.days.get(&date.to_string()).copied().unwrap_or(0.0);
        let col = i / 7;
        let row = i % 7;
        let tile = egui::Rect::from_min_size(
            rect.min + vec2(col as f32 * (cell + 4.0), row as f32 * (cell + 4.0)),
            vec2(cell, cell),
        );
        let fill = if ms > 0.0 {
            p.accent.gamma_multiply((0.25 + 0.75 * ms / max) as f32)
        } else {
            p.surface
        };
        ui.painter().rect_filled(tile, 3.0, fill);
        ui.interact(
            tile,
            ui.id().with(("listening-day", i)),
            egui::Sense::hover(),
        )
        .on_hover_text(format!("{date} · {}", listening::report::clock(ms)));
    }
    ui.label(
        RichText::new(format!(
            "{start} — {end} · brighter means more listening · hover any day"
        ))
        .small()
        .color(p.secondary),
    );
}
fn ranking(ui: &mut egui::Ui, p: &theme::Palette, title: &str, rows: &[Group], note: &str) {
    heading(ui, title);
    ui.label(RichText::new(note).small().color(p.secondary));
    ui.add_space(12.0);
    egui::ScrollArea::vertical()
        .max_height(650.0)
        .id_salt(title)
        .show_rows(ui, 55.0, rows.len(), |ui, range| {
            for i in range {
                let r = &rows[i];
                bar(
                    ui,
                    p,
                    &format!("{}. {}", i + 1, r.name),
                    &format!(
                        "{} · {} plays · {} songs",
                        listening::report::clock(r.listen_ms),
                        r.plays,
                        r.tracks
                    ),
                    r.listen_ms,
                    rows.first().map_or(1.0, |v| v.listen_ms),
                );
            }
        });
}
fn songs(app: &mut App, ui: &mut egui::Ui, r: &Report) {
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.listening_view.search)
                .hint_text("Find a song, artist, album, genre or tag…")
                .desired_width(330.0),
        );
        let labels = [
            "Listening time",
            "Play count",
            "Skip count",
            "Completion",
            "Most recent",
            "Title",
            "Star rating",
        ];
        egui::ComboBox::from_id_salt("stats-sort")
            .selected_text(labels[app.listening_view.sort])
            .show_ui(ui, |ui| {
                for (i, label) in labels.iter().enumerate() {
                    ui.selectable_value(&mut app.listening_view.sort, i, *label);
                }
            });
    });
    ui.add_space(12.0);
    let search = app.listening_view.search.to_lowercase();
    let mut rows: Vec<_> = r
        .tracks
        .iter()
        .filter(|r| {
            format!(
                "{} {} {} {} {}",
                r.name, r.subtitle, r.album, r.genres, r.tags
            )
            .to_lowercase()
            .contains(&search)
        })
        .collect();
    rows.sort_by(|a, b| match app.listening_view.sort {
        1 => b.plays.cmp(&a.plays),
        2 => b.skips.cmp(&a.skips),
        3 => b.completed.cmp(&a.completed),
        4 => b.last.cmp(&a.last),
        5 => a.name.cmp(&b.name),
        6 => b.rating.cmp(&a.rating),
        _ => b.listen_ms.total_cmp(&a.listen_ms),
    });
    ui.label(
        RichText::new(format!(
            "{} songs · click a title for details, genres, tags and your rating",
            rows.len()
        ))
        .small()
        .color(app.palette.secondary),
    );
    egui::ScrollArea::horizontal()
        .id_salt("stats-song-horizontal")
        .show(ui, |ui| {
            ui.set_min_width(1120.0);
            ui.horizontal(|ui| {
                for (label, w) in [
                    ("SONG / ARTIST", 290.0),
                    ("TIME", 92.0),
                    ("PLAYS", 58.0),
                    ("SKIPS", 58.0),
                    ("DONE", 58.0),
                    ("COVERAGE", 85.0),
                    ("FIRST PLAY", 150.0),
                    ("LAST PLAY", 150.0),
                ] {
                    ui.add_sized(
                        [w, 25.0],
                        egui::Label::new(RichText::new(label).small().color(app.palette.secondary)),
                    );
                }
            });
            egui::ScrollArea::vertical()
                .max_height(550.0)
                .id_salt("stats-song-rows")
                .show_rows(ui, 50.0, rows.len(), |ui, range| {
                    for i in range {
                        let row = rows[i];
                        ui.push_id(&row.id, |ui| {
                            ui.horizontal(|ui| {
                                ui.allocate_ui_with_layout(
                                    vec2(290.0, 44.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_min_width(290.0);
                                        ui.set_max_width(290.0);
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(&row.name).strong(),
                                                )
                                                .frame(false),
                                            )
                                            .clicked()
                                        {
                                            app.listening_view.selected = Some(row.id.clone());
                                            app.listening_view.genres = row.genres.clone();
                                            app.listening_view.tags = row.tags.clone();
                                            app.listening_view.rating = row.rating;
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&row.subtitle)
                                                    .small()
                                                    .color(app.palette.secondary),
                                            )
                                            .truncate(),
                                        );
                                    },
                                );
                                for (value, w) in [
                                    (listening::report::clock(row.listen_ms), 92.0),
                                    (row.plays.to_string(), 58.0),
                                    (row.skips.to_string(), 58.0),
                                    (row.completed.to_string(), 58.0),
                                    (
                                        if row.known_duration > 0 {
                                            format!(
                                                "{:.0}%",
                                                100.0 * row.coverage / row.known_duration as f64
                                            )
                                        } else {
                                            "—".into()
                                        },
                                        85.0,
                                    ),
                                    (local_date(row.first), 150.0),
                                    (local_date(row.last), 150.0),
                                ] {
                                    ui.add_sized([w, 44.0], egui::Label::new(value));
                                }
                            });
                        });
                    }
                });
        });
}
fn editor(app: &mut App, ctx: &egui::Context) {
    let Some(id) = app.listening_view.selected.clone() else {
        return;
    };
    let row = app
        .listening
        .report
        .as_ref()
        .and_then(|r| r.tracks.iter().find(|t| t.id == id))
        .cloned();
    let Some(row) = row else {
        return;
    };
    let mut open = true;
    let mut save = false;
    egui::Window::new("Song details")
        .id(egui::Id::new("listening-song-editor"))
        .open(&mut open)
        .default_width(440.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .collapsible(false)
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(RichText::new(&row.name).size(22.0).strong());
            ui.label(&row.subtitle);
            ui.label(&row.album);
            ui.add_space(12.0);
            ui.label(format!(
                "{} listened · {} plays · {} attempts · {} skips",
                listening::report::clock(row.listen_ms),
                row.plays,
                row.attempts,
                row.skips
            ));
            ui.label(format!(
                "First: {}\nLast: {}",
                local_date(row.first),
                local_date(row.last)
            ));
            ui.separator();
            ui.label("Genres (comma separated)");
            ui.text_edit_singleline(&mut app.listening_view.genres);
            ui.label("Your tags / moods (comma separated)");
            ui.text_edit_singleline(&mut app.listening_view.tags);
            ui.horizontal(|ui| {
                ui.label("Your rating");
                ui.add(egui::Slider::new(&mut app.listening_view.rating, 0..=5).suffix(" / 5"));
            });
            ui.label(
                RichText::new(
                    "Saved locally. These labels update your statistics across all dates.",
                )
                .small(),
            );
            save = ui.button("Save labels & rating").clicked();
        });
    if save {
        app.listening.tags(
            id,
            app.listening_view.genres.clone(),
            app.listening_view.tags.clone(),
            app.listening_view.rating,
        );
        if let Ok(q) = app.listening_view.query() {
            app.listening.request(q, true);
        }
        open = false;
    }
    if !open {
        app.listening_view.selected = None;
    }
}
fn history(ui: &mut egui::Ui, p: &theme::Palette, r: &Report) {
    ui.label(RichText::new("Every attempt, including partial plays and failures. Showing the latest 1,000 in this range; full history is preserved in exports.").color(p.secondary));
    egui::ScrollArea::vertical()
        .max_height(650.0)
        .id_salt("listening-history")
        .show_rows(ui, 77.0, r.recent.len(), |ui, range| {
            for i in range {
                let row = &r.recent[i];
                ui.push_id(i, |ui| {
                    ui.label(
                        RichText::new(format!("{}  ·  {}", local_date(row.at), row.title)).strong(),
                    );
                    ui.label(format!(
                        "{} · {} listened · {:.0}% covered · {} · {}",
                        row.artist,
                        listening::report::clock(row.listen_ms),
                        row.coverage * 100.0,
                        row.outcome,
                        row.source
                    ));
                    let context: listening::Context =
                        serde_json::from_str(&row.context).unwrap_or_default();
                    ui.label(
                        RichText::new(format!(
                            "{} · Repeat {} · {}",
                            if context.shuffle {
                                "Shuffle on"
                            } else {
                                "In order"
                            },
                            context.repeat,
                            if context.video {
                                "Music video"
                            } else {
                                "Audio"
                            }
                        ))
                        .small()
                        .color(p.secondary),
                    );
                });
            }
        });
}
fn recap(ui: &mut egui::Ui, p: &theme::Palette, r: &Report) {
    egui::Frame::new()
        .fill(p.surface)
        .corner_radius(18.0)
        .inner_margin(28.0)
        .show(ui, |ui| {
            ui.label(
                RichText::new("MOOSIK / YOUR RECAP")
                    .size(12.0)
                    .color(p.accent_text())
                    .strong(),
            );
            ui.add_space(12.0);
            ui.label(RichText::new(&r.query.label).size(32.0).strong());
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "{} with your music.",
                    listening::report::clock(r.listen_ms)
                ))
                .size(26.0),
            );
            ui.label(format!(
                "{} plays. {} songs. {} artists. {} days you pressed play.",
                r.plays,
                r.tracks.iter().filter(|t| t.listen_ms > 0.0).count(),
                r.artists.len(),
                r.active_days
            ));
            ui.add_space(20.0);
            if let Some(artist) = r.artists.first() {
                ui.label(
                    RichText::new(format!("Your #1 artist: {}", artist.name))
                        .size(22.0)
                        .strong(),
                );
                ui.label(format!(
                    "{} together · {} songs",
                    listening::report::clock(artist.listen_ms),
                    artist.tracks
                ));
            }
            if let Some(song) = r.tracks.iter().find(|s| s.listen_ms > 0.0) {
                ui.add_space(12.0);
                ui.label(
                    RichText::new(format!("Your #1 song: {}", song.name))
                        .size(22.0)
                        .strong(),
                );
                ui.label(format!(
                    "{} · {} plays · {}",
                    song.subtitle,
                    song.plays,
                    listening::report::clock(song.listen_ms)
                ));
            }
            ui.add_space(18.0);
            ui.label(format!(
                "{} songs heard for the first time here. A longest streak of {} days.",
                r.new_tracks, r.longest_streak
            ));
            if let Some((day, time)) = r.days.iter().max_by(|a, b| a.1.total_cmp(b.1)) {
                ui.label(format!(
                    "Your biggest music day: {day} · {}",
                    listening::report::clock(*time)
                ));
            }
        });
    heading(ui, "Make it your own");
    ui.label("Choose any date range above: a year, a summer, a trip, or a single day. Export your recap as an editable document, song rankings as CSV, or every raw event as JSON to build something entirely your own.");
    ranking(
        ui,
        p,
        "The soundtrack",
        &r.artists.iter().take(10).cloned().collect::<Vec<_>>(),
        "Your top artists for this chapter.",
    );
}
fn local_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| {
            d.with_timezone(&Local)
                .format("%b %d, %Y %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}
