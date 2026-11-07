//! A private listening observatory: raw history, iTunes-style song columns and custom recaps.
use crate::{
    app::App,
    icons::Icon,
    listening::{self, Export, Group, Query, Report},
    theme, widgets,
};
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use egui::{Align2, Rect, RichText, Sense, pos2, vec2};
use std::sync::Arc;

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
    source_history_range: Option<(i64, i64)>,
    source_history: Vec<String>,
    search: String,
    sort: usize,
    selected: Option<String>,
    genres: String,
    tags: String,
    rating: i32,
    help: bool,
    pub(crate) share: bool,
    share_feedback: Option<&'static str>,
    group: Option<(Tab, String, String)>,
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

// Follow Library's page chrome and Home's music rows. Utilities live in menus; the
// overview is a summary, not a second rendering of every database column.
const GAP: f32 = 24.0;
const PERIODS: [(Period, &str); 7] = [
    (Period::Today, "Today"),
    (Period::Week, "Last 7 days"),
    (Period::Month, "Last 30 days"),
    (Period::CalendarMonth, "This month"),
    (Period::Year, "This year"),
    (Period::All, "All time"),
    (Period::Custom, "Custom dates"),
];
const TABS: [(Tab, &str); 7] = [
    (Tab::Overview, "Overview"),
    (Tab::Songs, "Songs"),
    (Tab::Artists, "Artists"),
    (Tab::Albums, "Albums"),
    (Tab::Genres, "Genres"),
    (Tab::History, "History"),
    (Tab::Recap, "My recap"),
];

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    app.listening.poll();
    let p = app.palette;
    crate::ui::page_scroll(app, "listening-stats-v2").show(ui, |ui| {
        crate::ui::page_title(ui, &p, "Your listening");
        let mut refresh = false;
        ui.horizontal_wrapped(|ui| {
            let period = PERIODS
                .iter()
                .find(|(v, _)| *v == app.listening_view.period)
                .unwrap()
                .1;
            if let Some(i) = dropdown(ui, &p, "stats-period", period, &PERIODS.map(|(_, s)| s)) {
                app.listening_view.period = PERIODS[i].0;
                refresh = true;
            }
            source_filter(app, ui, &p, &mut refresh);
            if widgets::outline_button(ui, &p, "Share my week").clicked() {
                app.listening_view.period = Period::Week;
                app.listening_view.source.clear();
                app.listening_view.tab = Tab::Recap;
                app.listening_view.share_feedback = None;
                app.listening_view.share = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let more = widgets::icon_button(ui, &p, Icon::Ellipsis, 20.0, false)
                    .on_hover_text("Export and more");
                utility_menu(app, ui, &more, &mut refresh);
                if widgets::icon_button(ui, &p, Icon::Info, 18.0, false)
                    .on_hover_text("About your stats")
                    .clicked()
                {
                    app.listening_view.help = true;
                }
            });
        });
        if app.listening_view.period == Period::Custom {
            if app.listening_view.from.is_empty() {
                app.listening_view.from =
                    Local::now().date_naive().with_day(1).unwrap().to_string();
                app.listening_view.to = Local::now().date_naive().to_string();
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                widgets::text_input(ui, &p, &mut app.listening_view.from, "YYYY-MM-DD", 120.0);
                ui.label(
                    RichText::new("to")
                        .font(theme::regular(12.0))
                        .color(p.secondary),
                );
                widgets::text_input(ui, &p, &mut app.listening_view.to, "YYYY-MM-DD", 120.0);
            });
        }
        ui.add_space(16.0);
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in TABS {
                if widgets::chip(ui, &p, label, app.listening_view.tab == tab).clicked() {
                    app.listening_view.tab = tab;
                    app.listening_view.group = None;
                }
            }
        });
        ui.add_space(GAP);
        let query = match app.listening_view.query() {
            Ok(q) => q,
            Err(e) => {
                ui.colored_label(p.warning, e);
                return;
            }
        };
        app.listening.request(query.clone(), refresh);
        if let Some(error) = &app.listening.error {
            ui.colored_label(p.danger, error);
        }
        if let Some(notice) = app.listening.notice.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(notice)
                        .color(p.accent_text())
                        .font(theme::regular(12.0)),
                );
                if widgets::icon_button(ui, &p, Icon::X, 14.0, false).clicked() {
                    app.listening.notice = None;
                }
            });
        }
        let Some(report) = app.listening.report.take() else {
            widgets::skeleton_rows(ui, &p, 4);
            return;
        };
        if report.query != query {
            widgets::skeleton_rows(ui, &p, 4);
        } else if report.attempts == 0 && report.listen_ms == 0.0 {
            app.listening_view.share = false;
            empty(
                ui,
                &p,
                if query.source.is_empty() {
                    "No listening in this period"
                } else {
                    "No listening for this source"
                },
                "Play something in Moosik, or choose another source or date range.",
            );
            let providers = relevant_providers(app, Some(&report));
            if app.listening_view.tab == Tab::Overview && providers.len() > 1 {
                most_used_provider(ui, &p, &report);
                provider_breakdown(ui, &p, &providers);
            }
        } else {
            match app.listening_view.tab {
                Tab::Overview => overview(app, ui, &report),
                Tab::Songs => songs(app, ui, &report),
                Tab::Artists => ranking(app, ui, &report, Tab::Artists),
                Tab::Albums => ranking(app, ui, &report, Tab::Albums),
                Tab::Genres => ranking(app, ui, &report, Tab::Genres),
                Tab::History => history(app, ui, &report),
                Tab::Recap => recap(app, ui, &report),
            }
        }
        app.listening.report = Some(report);
        ui.add_space(theme::PAGE_INSET_X);
    });
    editor(app, ui.ctx());
    help(app, ui.ctx());
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs(1));
}

fn relevant_providers(app: &App, report: Option<&Report>) -> Vec<Group> {
    let mut providers: Vec<_> = report
        .into_iter()
        .flat_map(|report| &report.providers)
        .filter(|provider| provider.listen_ms.is_finite() && provider.listen_ms > 0.0)
        .cloned()
        .collect();
    for provider in app.connected_providers() {
        let id = match provider {
            crate::model::Provider::YouTubeMusic => "youtube",
            crate::model::Provider::Spotify => "spotify",
            crate::model::Provider::Local => "local",
        };
        if !providers.iter().any(|provider| provider.id == id) {
            providers.push(Group {
                id: id.to_owned(),
                name: provider.name().to_owned(),
                ..Group::default()
            });
        }
    }
    providers.sort_by(|left, right| {
        right
            .listen_ms
            .total_cmp(&left.listen_ms)
            .then_with(|| left.name.cmp(&right.name))
    });
    providers
}

fn source_filter(app: &mut App, ui: &mut egui::Ui, p: &theme::Palette, refresh: &mut bool) {
    let range = app
        .listening_view
        .query()
        .ok()
        .map(|query| (query.from, query.to));
    let report = app
        .listening
        .report
        .clone()
        .filter(|report| range == Some((report.query.from, report.query.to)));
    if app.listening_view.source_history_range != range {
        app.listening_view.source_history_range = range;
        app.listening_view.source_history.clear();
    }
    if let Some(report) = &report {
        if report.query.source.is_empty() {
            app.listening_view.source_history = report
                .sources
                .iter()
                .map(|source| source.id.clone())
                .collect();
        } else {
            for source in &report.sources {
                if !app.listening_view.source_history.contains(&source.id) {
                    app.listening_view.source_history.push(source.id.clone());
                }
            }
        }
    }
    let providers = relevant_providers(app, report.as_deref());
    let mut options = vec![(String::new(), "All sources".to_owned())];
    for provider in &providers {
        if provider.id.is_empty() {
            continue;
        }
        let label = match provider.id.as_str() {
            "youtube" => "YouTube Music streams",
            "spotify" => "Spotify",
            "local" => "Local files",
            _ => provider.name.as_str(),
        };
        options.push((provider.id.clone(), label.to_owned()));
    }
    if providers.iter().any(|provider| provider.id == "youtube")
        && (!app.downloads.entries.is_empty()
            || app
                .listening_view
                .source_history
                .iter()
                .any(|source| source == "downloaded"))
    {
        options.push((
            "downloaded".to_owned(),
            "Downloaded YouTube Music".to_owned(),
        ));
    }
    let can_filter = options.len() > 2;
    if report.is_some()
        && (!can_filter
            || !options
                .iter()
                .any(|(id, _)| id == &app.listening_view.source))
        && !app.listening_view.source.is_empty()
    {
        app.listening_view.source.clear();
        *refresh = true;
    }
    if !can_filter && (app.listening_view.source.is_empty() || report.is_some()) {
        return;
    }
    if !options
        .iter()
        .any(|(id, _)| id == &app.listening_view.source)
    {
        options.push((
            app.listening_view.source.clone(),
            source_label(&app.listening_view.source).to_owned(),
        ));
    }
    let selected = options
        .iter()
        .position(|(id, _)| id == &app.listening_view.source)
        .unwrap_or(0);
    let labels: Vec<_> = options.iter().map(|(_, label)| label.as_str()).collect();
    if let Some(index) = dropdown(ui, p, "stats-source", &options[selected].1, &labels) {
        app.listening_view.source = options[index].0.clone();
        *refresh = true;
    }
}

fn dropdown(
    ui: &mut egui::Ui,
    p: &theme::Palette,
    id: &str,
    current: &str,
    options: &[&str],
) -> Option<usize> {
    let response = widgets::outline_button(ui, p, &format!("{current}  ⌄"));
    let width = options
        .iter()
        .map(|label| widgets::text_width(ui, label, &theme::medium(13.0)) + 48.0)
        .fold(190.0, f32::max)
        .min((ui.ctx().content_rect().width() - theme::PAGE_INSET_X * 2.0).max(0.0));
    let mut choice = None;
    egui::Popup::from_response(&response)
        .id(egui::Id::new(id))
        .kind(egui::PopupKind::Menu)
        .frame(widgets::menu_frame(ui, p))
        .width(width)
        .gap(4.0)
        .open_memory(if response.clicked() {
            Some(egui::SetOpenCommand::Toggle)
        } else {
            None
        })
        .show(|ui| {
            for (i, label) in options.iter().enumerate() {
                if widgets::menu_item_plain(
                    ui,
                    p,
                    label,
                    if *label == current { Some("✓") } else { None },
                )
                .clicked()
                {
                    choice = Some(i);
                    ui.close();
                }
            }
        });
    choice
}
fn utility_menu(app: &mut App, ui: &mut egui::Ui, response: &egui::Response, refresh: &mut bool) {
    let p = app.palette;
    let mut export = None;
    egui::Popup::from_response(response)
        .id(egui::Id::new("stats-utilities"))
        .kind(egui::PopupKind::Menu)
        .frame(widgets::menu_frame(ui, &p))
        .width(220.0)
        .align(egui::RectAlign::BOTTOM_END)
        .open_memory(if response.clicked() {
            Some(egui::SetOpenCommand::Toggle)
        } else {
            None
        })
        .show(|ui| {
            if widgets::menu_item(ui, &p, Icon::RefreshCw, "Refresh", None).clicked() {
                *refresh = true;
                ui.close();
            }
            widgets::menu_divider(ui, &p);
            for (kind, label) in [
                (Export::Recap, "Export recap"),
                (Export::Csv, "Export song stats"),
                (Export::Json, "Export all history"),
                (Export::Database, "Back up database"),
            ] {
                if widgets::menu_item(ui, &p, Icon::Download, label, None).clicked() {
                    export = Some(kind);
                    ui.close();
                }
            }
        });
    if let Some(kind) = export {
        export_file(app, kind);
    }
}
fn export_file(app: &mut App, kind: Export) {
    let (label, ext) = match kind {
        Export::Recap => ("Recap", "md"),
        Export::Csv => ("Song stats", "csv"),
        Export::Json => ("All listening history", "json"),
        Export::Database => ("Database backup", "sqlite3"),
    };
    if let Ok(q) = app.listening_view.query()
        && let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("moosik-{}.{ext}", Local::now().format("%Y-%m-%d")))
            .add_filter(label, &[ext])
            .save_file()
    {
        app.listening.export(kind, path, q);
    }
}
fn empty(ui: &mut egui::Ui, p: &theme::Palette, title: &str, detail: &str) {
    ui.add_space(32.0);
    ui.vertical_centered(|ui| {
        ui.add(Icon::Music.image(p.dim, 36.0));
        ui.add_space(16.0);
        ui.label(
            RichText::new(title)
                .font(theme::semibold(18.0))
                .color(p.text),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(detail)
                .font(theme::regular(13.0))
                .color(p.secondary),
        );
    });
    ui.add_space(40.0);
}
fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}
fn clock(ms: f64) -> String {
    listening::report::clock(ms)
}
fn columns(ui: &mut egui::Ui, count: usize, body: impl FnOnce(&mut [egui::Ui])) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        ui.columns(count, body);
    });
}
fn small(ui: &mut egui::Ui, p: &theme::Palette, text: impl Into<String>) {
    ui.label(
        RichText::new(text)
            .font(theme::regular(12.0))
            .color(p.secondary),
    );
}
fn section(ui: &mut egui::Ui, p: &theme::Palette, title: &str, action: Option<&str>) -> bool {
    ui.add_space(20.0);
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title)
                .font(theme::bold(theme::SECTION_PT))
                .color(p.text),
        );
        if let Some(action) = action {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clicked = ui
                    .add(
                        egui::Button::new(
                            RichText::new(action)
                                .font(theme::medium(12.0))
                                .color(p.secondary),
                        )
                        .frame(false),
                    )
                    .clicked();
            });
        }
    });
    ui.add_space(8.0);
    clicked
}
fn metric(ui: &mut egui::Ui, p: &theme::Palette, label: &str, value: &str) {
    ui.label(
        RichText::new(value)
            .font(theme::semibold(24.0))
            .color(p.text),
    );
    small(ui, p, label);
}
fn overview(app: &mut App, ui: &mut egui::Ui, r: &Report) {
    let p = app.palette;
    let providers = relevant_providers(app, Some(r));
    let compare_providers = providers.len() > 1;
    // One dominant number and a quiet activity chart; no dashboard of eight equal cards.
    egui::Frame::new()
        .fill(p.glass_state(p.surface, 0.35))
        .corner_radius(theme::ART_RADIUS_MAX)
        .inner_margin(20.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if ui.available_width() > 680.0 {
                columns(ui, 2, |cols| {
                    summary(&mut cols[0], &p, r, compare_providers);
                    activity(&mut cols[1], &p, r);
                });
            } else {
                summary(ui, &p, r, compare_providers);
                ui.add_space(20.0);
                activity(ui, &p, r);
            }
        });
    ui.add_space(16.0);
    columns(ui, 3, |cols| {
        metric(&mut cols[0], &p, "Plays", &r.plays.to_string());
        metric(
            &mut cols[1],
            &p,
            "Songs",
            &r.tracks
                .iter()
                .filter(|t| t.listen_ms > 0.0)
                .count()
                .to_string(),
        );
        metric(&mut cols[2], &p, "Artists", &r.artists.len().to_string());
    });
    if ui.available_width() > 720.0 {
        columns(ui, 2, |cols| {
            top_songs(app, &mut cols[0], r);
            top_artists(app, &mut cols[1], r);
        });
    } else {
        top_songs(app, ui, r);
        top_artists(app, ui, r);
    }
    if r.query.from > 0 && r.previous_ms > 0.0 {
        let mut rising: Vec<_> = r
            .artists
            .iter()
            .filter_map(|artist| {
                let previous = r
                    .previous_artists
                    .iter()
                    .find(|a| a.id == artist.id)
                    .map_or(0.0, |a| a.listen_ms);
                let gain = artist.listen_ms - previous;
                (gain >= 60_000.0).then_some((artist, gain))
            })
            .collect();
        rising.sort_by(|a, b| b.1.total_cmp(&a.1));
        if !rising.is_empty() {
            section(ui, &p, "On the rise", None);
            small(ui, &p, "Compared with the previous equal-length period");
            ui.add_space(8.0);
            for (artist, gain) in rising.into_iter().take(3) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(&artist.name)
                            .font(theme::medium(14.0))
                            .color(p.text),
                    );
                    ui.label(
                        RichText::new(format!("+{} listening", clock(gain)))
                            .font(theme::regular(12.0))
                            .color(p.accent),
                    );
                });
            }
        }
    }
    provider_breakdown(ui, &p, &providers);
    section(ui, &p, "Listening habits", None);
    if ui.available_width() > 640.0 {
        columns(ui, 2, |cols| {
            small(&mut cols[0], &p, "Time of day");
            hour_chart(&mut cols[0], &p, r);
            small(&mut cols[1], &p, "Day of week");
            week_chart(&mut cols[1], &p, r);
        });
    } else {
        small(ui, &p, "Time of day");
        hour_chart(ui, &p, r);
        ui.add_space(12.0);
        small(ui, &p, "Day of week");
        week_chart(ui, &p, r);
    }
    ui.add_space(16.0);
    columns(ui, 3, |cols| {
        metric(&mut cols[0], &p, "Active days", &r.active_days.to_string());
        metric(
            &mut cols[1],
            &p,
            "Longest streak",
            &count(r.longest_streak, "day"),
        );
        metric(&mut cols[2], &p, "New songs", &r.new_tracks.to_string());
    });
    ui.add_space(GAP);
    egui::CollapsingHeader::new(
        RichText::new("More listening details")
            .font(theme::medium(13.0))
            .color(p.secondary),
    )
    .show(ui, |ui| {
        detail_pairs(
            ui,
            &p,
            &[
                ("Completed listens", r.completed.to_string()),
                ("Skipped tracks", r.skips.to_string()),
                (
                    "Seek events",
                    r.events.get("seek").unwrap_or(&0).to_string(),
                ),
                ("Play attempts", r.attempts.to_string()),
                (
                    "Average / active day",
                    clock(r.listen_ms / r.active_days.max(1) as f64),
                ),
                ("Paused", clock(r.paused_ms)),
                ("Buffering", clock(r.buffer_ms)),
                ("App muted", clock(r.muted_ms)),
                ("Unobserved", clock(r.unobserved_ms)),
            ],
        );
        ui.add_space(12.0);
        for source in &r.sources {
            detail_pairs(
                ui,
                &p,
                &[(source_label(&source.name), clock(source.listen_ms))],
            );
        }
        if r.query.from > 0 && r.previous_ms > 0.0 {
            small(
                ui,
                &p,
                format!("Previous equal-length period: {}", clock(r.previous_ms)),
            );
        }
        ui.add_space(16.0);
        heatmap(ui, &p, r);
    });
}
fn summary(ui: &mut egui::Ui, p: &theme::Palette, r: &Report, compare_providers: bool) {
    small(ui, p, "Listening time");
    ui.add_space(6.0);
    ui.label(
        RichText::new(clock(r.listen_ms))
            .font(theme::bold(42.0))
            .color(p.text),
    );
    ui.add_space(10.0);
    small(
        ui,
        p,
        format!("{} · {}", r.query.label, count(r.active_days, "active day")),
    );
    if compare_providers {
        most_used_provider(ui, p, r);
    }
    if r.query.from > 0 && r.previous_ms > 0.0 {
        ui.add_space(6.0);
        let change = (r.listen_ms / r.previous_ms - 1.0) * 100.0;
        small(
            ui,
            p,
            format!(
                "{change:+.0}% vs previous period · {} before",
                clock(r.previous_ms)
            ),
        );
    }
}
fn most_used_provider(ui: &mut egui::Ui, p: &theme::Palette, r: &Report) {
    let providers: Vec<_> = r
        .providers
        .iter()
        .filter(|provider| provider.listen_ms.is_finite() && provider.listen_ms > 0.0)
        .collect();
    let total: f64 = providers.iter().map(|provider| provider.listen_ms).sum();
    let Some(top) = providers
        .iter()
        .max_by(|left, right| left.listen_ms.total_cmp(&right.listen_ms))
    else {
        return;
    };
    let leaders: Vec<_> = providers
        .iter()
        .filter(|provider| provider.listen_ms == top.listen_ms)
        .map(|provider| provider.name.as_str())
        .collect();
    let tied = leaders.len() > 1;
    let name = leaders.join(" · ");
    ui.add_space(12.0);
    small(
        ui,
        p,
        if tied {
            "Top providers tied"
        } else {
            "Most-used provider"
        },
    );
    if !r.query.source.is_empty() {
        small(ui, p, "Across all providers for these dates");
    }
    ui.add(egui::Label::new(RichText::new(&name).font(theme::semibold(18.0)).color(p.text)).truncate())
        .on_hover_text(format!("{name}\nBased on recorded listening time across all providers in the selected period. YouTube Music includes downloaded songs."));
    small(
        ui,
        p,
        format!(
            "{} · {} of listening time{}",
            clock(top.listen_ms),
            source_share(top.listen_ms, total),
            if tied { " each" } else { "" }
        ),
    );
}

fn source_share(time: f64, total: f64) -> String {
    if total <= 0.0 || time <= 0.0 {
        return "0%".to_owned();
    }
    let share = (time / total * 100.0).clamp(0.0, 100.0);
    if share < 1.0 {
        "<1%".to_owned()
    } else {
        format!("{share:.0}%")
    }
}

fn provider_breakdown(ui: &mut egui::Ui, p: &theme::Palette, providers: &[Group]) {
    if providers.len() <= 1 {
        return;
    }
    section(ui, p, "Listening by provider", None);
    small(
        ui,
        p,
        "All providers for the selected dates, regardless of the source filter.",
    );
    let total: f64 = providers
        .iter()
        .map(|provider| provider.listen_ms.max(0.0))
        .sum();
    ui.add_space(8.0);
    for provider in providers {
        let share = source_share(provider.listen_ms, total);
        let (rect, response) = ui.allocate_exact_size(
            vec2(ui.available_width(), theme::ROW_HEIGHT),
            Sense::hover(),
        );
        let icon_rect = Rect::from_center_size(
            pos2(rect.left() + 10.0, rect.top() + 15.0),
            vec2(20.0, 20.0),
        );
        widgets::source_name_indicator(
            ui,
            p,
            icon_rect,
            &provider.name,
            ui.id().with(("stats-provider-icon", &provider.id)),
        );
        let value = format!("{} · {share}", clock(provider.listen_ms));
        let value_width = widgets::text_width(ui, &value, &theme::regular(13.0));
        let name_rect = Rect::from_min_max(
            pos2(icon_rect.right() + 8.0, rect.top()),
            pos2(
                (rect.right() - value_width - 12.0).max(icon_rect.right() + 8.0),
                rect.top() + 30.0,
            ),
        );
        clipped(ui, &provider.name, name_rect, theme::medium(13.0), p.text);
        ui.painter().text(
            pos2(rect.right(), rect.top() + 15.0),
            Align2::RIGHT_CENTER,
            &value,
            theme::regular(13.0),
            p.secondary,
        );
        let bar = Rect::from_min_size(
            pos2(rect.left(), rect.top() + 36.0),
            vec2(rect.width(), 4.0),
        );
        ui.painter().rect_filled(bar, 2.0, p.surface);
        if total > 0.0 && provider.listen_ms > 0.0 {
            let fill = Rect::from_min_size(
                bar.min,
                vec2(
                    bar.width() * (provider.listen_ms / total).clamp(0.0, 1.0) as f32,
                    bar.height(),
                ),
            );
            ui.painter().rect_filled(fill, 2.0, p.accent);
        }
        response.on_hover_text(format!(
            "{} · {} · {} qualified plays · {share} of recorded listening time{}",
            provider.name,
            clock(provider.listen_ms),
            provider.plays,
            if provider.id == "youtube" {
                "\nIncludes streamed and downloaded YouTube Music songs."
            } else {
                ""
            }
        ));
    }
}
fn activity(ui: &mut egui::Ui, p: &theme::Palette, r: &Report) {
    let end = chrono::DateTime::from_timestamp_millis(r.query.to - 1)
        .map(|v| v.with_timezone(&Local).date_naive())
        .unwrap_or_else(|| Local::now().date_naive());
    let start = end - chrono::Days::new(29);
    let values: Vec<_> = (0..30)
        .map(|n| {
            let d = start + chrono::Days::new(n);
            (
                d.format("%b %-d").to_string(),
                *r.days.get(&d.to_string()).unwrap_or(&0.0),
            )
        })
        .collect();
    small(ui, p, "Daily listening");
    histogram(ui, p, &values, 72.0, 0);
    ui.horizontal(|ui| {
        small(ui, p, start.format("%b %-d").to_string());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            small(ui, p, end.format("%b %-d").to_string())
        });
    });
}
fn hour_chart(ui: &mut egui::Ui, p: &theme::Palette, r: &Report) {
    let v = r
        .hours
        .iter()
        .enumerate()
        .map(|(h, v)| (format!("{h:02}:00"), *v))
        .collect::<Vec<_>>();
    histogram(ui, p, &v, 70.0, 6);
}
fn week_chart(ui: &mut egui::Ui, p: &theme::Palette, r: &Report) {
    let v = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        .iter()
        .zip(r.weekdays)
        .map(|(s, v)| (s.to_string(), v))
        .collect::<Vec<_>>();
    histogram(ui, p, &v, 70.0, 1);
}
fn histogram(
    ui: &mut egui::Ui,
    p: &theme::Palette,
    values: &[(String, f64)],
    height: f32,
    label_step: usize,
) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            ui.available_width(),
            height + if label_step > 0 { 24.0 } else { 4.0 },
        ),
        Sense::hover(),
    );
    let max = values.iter().map(|v| v.1).fold(1.0_f64, f64::max);
    let step = rect.width() / values.len().max(1) as f32;
    for (i, (label, value)) in values.iter().enumerate() {
        let x = rect.left() + i as f32 * step;
        let h = (*value / max) as f32 * height;
        let bar = Rect::from_min_max(
            pos2(x + 2.0, rect.top() + height - h.max(2.0)),
            pos2(x + step - 2.0, rect.top() + height),
        );
        ui.painter()
            .rect_filled(bar, 2.0, if *value > 0.0 { p.accent } else { p.surface });
        let hit = Rect::from_min_max(pos2(x, rect.top()), pos2(x + step, rect.top() + height));
        ui.interact(
            hit,
            ui.id().with(("stat-bar", i, rect.top().to_bits())),
            Sense::hover(),
        )
        .on_hover_text(format!("{label} · {}", clock(*value)));
        if label_step > 0 && i % label_step == 0 {
            ui.painter().text(
                pos2(x + step / 2.0, rect.bottom() - 6.0),
                Align2::CENTER_CENTER,
                label,
                theme::regular(10.0),
                p.dim,
            );
        }
    }
}
fn heatmap(ui: &mut egui::Ui, p: &theme::Palette, r: &Report) {
    small(ui, p, "Activity calendar");
    let end = chrono::DateTime::from_timestamp_millis(r.query.to - 1)
        .map(|d| d.with_timezone(&Local).date_naive())
        .unwrap_or_else(|| Local::now().date_naive());
    let start = end - chrono::Days::new(90);
    let cell = ((ui.available_width() / 13.0) - 4.0).clamp(6.0, 12.0);
    let max = r.days.values().copied().fold(1.0_f64, f64::max);
    let (rect, _) = ui.allocate_exact_size(
        vec2(13.0 * (cell + 4.0), 7.0 * (cell + 4.0)),
        Sense::hover(),
    );
    for i in 0..91 {
        let d = start + chrono::Days::new(i);
        let ms = r.days.get(&d.to_string()).copied().unwrap_or(0.0);
        let tile = Rect::from_min_size(
            rect.min + vec2((i / 7) as f32 * (cell + 4.0), (i % 7) as f32 * (cell + 4.0)),
            vec2(cell, cell),
        );
        ui.painter().rect_filled(
            tile,
            2.0,
            if ms > 0.0 {
                p.accent.gamma_multiply((0.25 + 0.75 * ms / max) as f32)
            } else {
                p.surface
            },
        );
        ui.interact(tile, ui.id().with(("day", i)), Sense::hover())
            .on_hover_text(format!("{d} · {}", clock(ms)));
    }
}
fn art(app: &mut App, ui: &egui::Ui, rect: Rect, url: Option<&str>) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let url = url.map(Arc::<str>::from);
    let texture = crate::ui::art_texture(
        &mut app.images,
        &app.backend,
        ui.ctx(),
        url.as_ref(),
        rect.width(),
    );
    widgets::paint_art(ui, &app.palette, rect, texture, widgets::ArtShape::Square);
}
fn clipped(ui: &egui::Ui, text: &str, rect: Rect, font: egui::FontId, color: egui::Color32) {
    if rect.width() <= 0.0 {
        return;
    }
    let text = widgets::truncate_to(ui, text, font.clone(), rect.width());
    ui.painter()
        .with_clip_rect(rect.intersect(ui.clip_rect()))
        .text(rect.left_center(), Align2::LEFT_CENTER, text, font, color);
}
fn row_hover(ui: &egui::Ui, p: &theme::Palette, rect: Rect, response: &egui::Response) {
    if response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect, theme::RADIUS, p.glass(p.surface_hover));
    }
}
fn select_song(app: &mut App, row: &listening::Row) {
    app.listening_view.selected = Some(row.id.clone());
    app.listening_view.genres = row.genres.clone();
    app.listening_view.tags = row.tags.clone();
    app.listening_view.rating = row.rating;
}
fn top_songs(app: &mut App, ui: &mut egui::Ui, r: &Report) {
    let p = app.palette;
    if section(ui, &p, "Top songs", Some("Show all")) {
        app.listening_view.tab = Tab::Songs;
        app.listening_view.group = None;
    }
    for (i, row) in r
        .tracks
        .iter()
        .filter(|r| r.listen_ms > 0.0)
        .take(5)
        .enumerate()
    {
        song_row(app, ui, row, i, false);
    }
}
fn top_artists(app: &mut App, ui: &mut egui::Ui, r: &Report) {
    let p = app.palette;
    if section(ui, &p, "Top artists", Some("Show all")) {
        app.listening_view.tab = Tab::Artists;
    }
    for (i, g) in r.artists.iter().take(5).enumerate() {
        group_row(app, ui, r, g, i, Tab::Artists);
    }
}
fn row_track(row: &listening::Row) -> crate::model::Track {
    row.metadata
        .clone()
        .unwrap_or_else(|| listening::Metadata {
            id: row.id.clone(),
            title: row.name.clone(),
            artists: vec![(row.subtitle.clone(), None)],
            album: (!row.album.is_empty()).then(|| row.album.clone()),
            artwork: row.artwork.clone(),
            ..Default::default()
        })
        .to_track()
}

/// Shared song component, with the stats columns and a separate details affordance beside it.
fn song_row(app: &mut App, ui: &mut egui::Ui, row: &listening::Row, index: usize, table: bool) {
    let p = app.palette;
    let track = row_track(row);
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::ROW_HEIGHT),
        Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    let listened = clock(row.listen_ms);
    let wide = table && rect.width() > 650.0;
    let medium = table && rect.width() > 460.0;
    let reserve = if !table {
        0.0
    } else if wide {
        285.0
    } else if medium {
        180.0
    } else {
        95.0
    };
    let cell = Rect::from_min_max(rect.min, pos2(rect.right() - reserve - 32.0, rect.bottom()));
    let style = if table {
        SongRow::Ranked
    } else {
        SongRow::Top(&listened)
    };
    regular_song(app, ui, &track, cell, index, style);
    let right = rect.right() - 44.0;
    if wide {
        value_at(ui, &p, rect, right - 210.0, &listened);
        value_at(ui, &p, rect, right - 138.0, &row.plays.to_string());
        value_at(ui, &p, rect, right - 72.0, &row.skips.to_string());
        value_at(ui, &p, rect, right, &row.completed.to_string());
    } else if medium {
        value_at(ui, &p, rect, right - 80.0, &listened);
        value_at(ui, &p, rect, right, &row.plays.to_string());
    } else if table {
        value_at(ui, &p, rect, right, &listened);
    }
    if details_button(ui, &p, rect, index, "View listening details").clicked() {
        select_song(app, row);
    }
}

fn details_button(
    ui: &mut egui::Ui,
    p: &theme::Palette,
    rect: Rect,
    index: usize,
    tooltip: &str,
) -> egui::Response {
    let rect = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(32.0, 32.0));
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt(("song-details", index))
            .max_rect(rect),
        |ui| widgets::icon_button(ui, p, Icon::Info, 16.0, false).on_hover_text(tooltip),
    )
    .inner
}

pub(crate) enum SongRow<'a> {
    Unnumbered,
    Ranked,
    Top(&'a str),
}

pub(crate) fn regular_song(
    app: &mut App,
    ui: &mut egui::Ui,
    track: &crate::model::Track,
    rect: Rect,
    index: usize,
    style: SongRow<'_>,
) {
    use crate::ui::{ListClick, RowSpec, is_active, track_subtitle};
    let signed_in = app.signed_in();
    let (click, action) = crate::ui::paint_grid_cell(
        ui,
        &app.palette,
        &mut app.images,
        &app.backend,
        &app.liked,
        rect,
        index,
        RowSpec {
            number: (!matches!(style, SongRow::Unnumbered)).then_some(index + 1),
            rank: None,
            trend: track.trend,
            title: &track.title,
            source: Some(track.source_name()),
            album_id: track.album_id.as_deref(),
            subtitle: track_subtitle(track),
            duration: track.duration,
            plays: None,
            explicit: track.explicit,
            active: is_active(&app.now, &track.video_id),
            playing: is_active(&app.now, &track.video_id) && !app.now.paused,
            badge: None,
            shape: widgets::ArtShape::Square,
            hide_art: false,
            thumbnail: track.thumbnail.as_ref(),
            menu: Some(widgets::RowMenu {
                track,
                playlist_id: None,
                playlists: &app.library.playlists.items,
                signed_in,
                queue_index: None,
            }),
            like: track.like,
            show_rating: true,
            quiet_like: matches!(style, SongRow::Top(_)),
            add_to: None,
        },
        match style {
            SongRow::Top(time) => Some(time),
            _ => None,
        },
    );
    if let Some(action) = action {
        app.row_action(action);
    }
    match click {
        Some(ListClick::Link(route)) => app.navigate(route),
        Some(ListClick::Row(_)) => {
            if is_active(&app.now, &track.video_id) {
                app.toggle_playback();
            } else {
                app.play_row(vec![track.clone()], 0);
            }
        }
        None => {}
    }
}

fn value_at(ui: &egui::Ui, p: &theme::Palette, rect: Rect, x: f32, value: &str) {
    ui.painter().text(
        pos2(x, rect.center().y),
        Align2::RIGHT_CENTER,
        value,
        theme::regular(12.0),
        p.secondary,
    );
}
fn group_matches(row: &listening::Row, kind: Tab, id: &str) -> bool {
    match kind {
        Tab::Artists => row.artist_keys.iter().any(|k| k == id),
        Tab::Albums => row.album_key == id,
        Tab::Genres => {
            if id.eq_ignore_ascii_case("Unknown") {
                row.genres.is_empty()
            } else {
                row.genres
                    .split(',')
                    .any(|g| g.trim().to_lowercase() == id.to_lowercase())
            }
        }
        _ => true,
    }
}
fn group_row(app: &mut App, ui: &mut egui::Ui, r: &Report, g: &Group, index: usize, kind: Tab) {
    let p = app.palette;
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::ROW_HEIGHT),
        Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        row_hover(ui, &p, rect, &response);
        ui.painter().text(
            pos2(rect.left() + 16.0, rect.center().y),
            Align2::CENTER_CENTER,
            (index + 1).to_string(),
            theme::regular(12.0),
            p.dim,
        );
        let cover = Rect::from_min_size(rect.min + vec2(36.0, 8.0), vec2(40.0, 40.0));
        if kind == Tab::Albums {
            let track = r.tracks.iter().find(|t| group_matches(t, kind, &g.id));
            art(app, ui, cover, track.and_then(|t| t.artwork.as_deref()));
        } else {
            ui.painter().rect_filled(
                cover,
                if kind == Tab::Artists {
                    20
                } else {
                    theme::RADIUS
                },
                p.glass(p.surface),
            );
            if kind == Tab::Artists {
                let initials = g
                    .name
                    .split_whitespace()
                    .take(2)
                    .filter_map(|s| s.chars().next())
                    .collect::<String>()
                    .to_uppercase();
                ui.painter().text(
                    cover.center(),
                    Align2::CENTER_CENTER,
                    initials,
                    theme::semibold(13.0),
                    p.secondary,
                );
            } else {
                Icon::Music
                    .image(p.secondary, 18.0)
                    .paint_at(ui, Rect::from_center_size(cover.center(), vec2(18.0, 18.0)));
            }
        }
        let title = Rect::from_min_size(
            pos2(cover.right() + 12.0, rect.top() + 8.0),
            vec2((rect.width() - 185.0).max(20.0), 20.0),
        );
        clipped(ui, &g.name, title, theme::medium(14.0), p.text);
        clipped(
            ui,
            &format!(
                "{} · {}",
                count(g.tracks, "song"),
                count(g.plays as usize, "play")
            ),
            title.translate(vec2(0.0, 20.0)),
            theme::regular(12.0),
            p.secondary,
        );
        value_at(ui, &p, rect, rect.right() - 12.0, &clock(g.listen_ms));
    }
    if response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Explore songs")
        .clicked()
    {
        app.listening_view.group = Some((kind, g.id.clone(), g.name.clone()));
        app.listening_view.tab = Tab::Songs;
        app.listening_view.search.clear();
    }
}
fn songs(app: &mut App, ui: &mut egui::Ui, r: &Report) {
    let p = app.palette;
    if let Some((_, _, name)) = app.listening_view.group.clone() {
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(RichText::new(name).font(theme::bold(theme::SECTION_PT)))
                    .truncate(),
            );
            if widgets::icon_chip(ui, &p, Icon::X, 16.0)
                .on_hover_text("Clear filter")
                .clicked()
            {
                app.listening_view.group = None;
            }
        });
        ui.add_space(12.0);
    }
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 48.0).clamp(100.0, 330.0);
        widgets::text_input(
            ui,
            &p,
            &mut app.listening_view.search,
            "Search songs, artists or tags",
            width,
        );
        let sorts = [
            "Listening time",
            "Play count",
            "Skip count",
            "Completion",
            "Most recent",
            "Title",
            "Star rating",
        ];
        if let Some(i) = widgets::icon_dropdown(
            ui,
            &p,
            egui::Id::new("stats-sort"),
            Icon::ArrowUpDown,
            false,
            sorts[app.listening_view.sort],
            &sorts,
        ) {
            app.listening_view.sort = i;
        }
    });
    ui.add_space(12.0);
    let search = app.listening_view.search.to_lowercase();
    let mut rows: Vec<_> = r
        .tracks
        .iter()
        .filter(|t| {
            let text = format!(
                "{} {} {} {} {}",
                t.name, t.subtitle, t.album, t.genres, t.tags
            )
            .to_lowercase();
            text.contains(&search)
                && app
                    .listening_view
                    .group
                    .as_ref()
                    .is_none_or(|(k, id, _)| group_matches(t, *k, id))
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
    small(ui, &p, count(rows.len(), "song"));
    ui.add_space(8.0);
    table_header(ui, &p);
    if rows.is_empty() {
        empty(
            ui,
            &p,
            "No matching songs",
            "Try a different search or clear the filter.",
        );
    }
    crate::ui::virtual_rows(ui, theme::ROW_HEIGHT, rows.len(), |ui, i| {
        song_row(app, ui, rows[i], i, true);
    });
}
fn table_header(ui: &mut egui::Ui, p: &theme::Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let right = rect.right() - 44.0;
    let label = |x: f32, text: &str, align: Align2| {
        ui.painter().text(
            pos2(x, rect.center().y),
            align,
            text,
            theme::medium(11.0),
            p.dim,
        );
    };
    label(rect.left() + 88.0, "Song", Align2::LEFT_CENTER);
    if rect.width() > 650.0 {
        label(right - 210.0, "Time", Align2::RIGHT_CENTER);
        label(right - 138.0, "Plays", Align2::RIGHT_CENTER);
        label(right - 72.0, "Skips", Align2::RIGHT_CENTER);
        label(right, "Finished", Align2::RIGHT_CENTER);
    } else if rect.width() > 460.0 {
        label(right - 80.0, "Time", Align2::RIGHT_CENTER);
        label(right, "Plays", Align2::RIGHT_CENTER);
    } else {
        label(right, "Time", Align2::RIGHT_CENTER);
    }
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        egui::Stroke::new(1.0, p.outline.gamma_multiply(0.3)),
    );
}
fn ranking(app: &mut App, ui: &mut egui::Ui, r: &Report, kind: Tab) {
    let p = app.palette;
    let rows = match kind {
        Tab::Artists => &r.artists,
        Tab::Albums => &r.albums,
        _ => &r.genres,
    };
    if kind == Tab::Genres && r.unknown_genre_ms > 0.0 {
        ui.horizontal_wrapped(|ui| {
            small(ui, &p, format!("{} untagged", clock(r.unknown_genre_ms)));
            if widgets::outline_button(ui, &p, "Tag songs").clicked() {
                app.listening_view.group =
                    Some((Tab::Genres, "Unknown".into(), "Untagged songs".into()));
                app.listening_view.tab = Tab::Songs;
                app.listening_view.search.clear();
            }
        });
        ui.add_space(12.0);
    }
    if rows.is_empty() {
        empty(
            ui,
            &p,
            "Nothing here yet",
            "Your listening will appear here as you play.",
        );
        return;
    }
    crate::ui::virtual_rows(ui, theme::ROW_HEIGHT, rows.len(), |ui, i| {
        group_row(app, ui, r, &rows[i], i, kind)
    });
}
fn detail_pairs(ui: &mut egui::Ui, p: &theme::Palette, pairs: &[(&str, String)]) {
    for (label, value) in pairs {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
        clipped(
            ui,
            label,
            Rect::from_min_size(rect.min, vec2(rect.width() * 0.55, rect.height())),
            theme::regular(12.0),
            p.secondary,
        );
        ui.painter().text(
            rect.right_center(),
            Align2::RIGHT_CENTER,
            value,
            theme::medium(12.0),
            p.text,
        );
    }
}
fn modal_frame(p: &theme::Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.overlay)
        .corner_radius(theme::RADIUS + 2)
        .inner_margin(24.0)
        .shadow(egui::epaint::Shadow {
            offset: [0, 8],
            blur: 28,
            spread: 0,
            color: p.shadow,
        })
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
    let p = app.palette;
    let mut close = false;
    let mut save = false;
    let width = (ctx.content_rect().width() - 80.0).clamp(240.0, 430.0);
    let modal_id = egui::Id::new("stats-song-detail");
    let modal = egui::Modal::new(modal_id)
        .area(
            egui::Modal::default_area(modal_id)
                .default_size(vec2(width + 48.0, ctx.content_rect().height() - 80.0)),
        )
        .frame(modal_frame(&p))
        .backdrop_color(egui::Color32::from_black_alpha(150))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                small(ui, &p, "Song details");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = widgets::icon_button(ui, &p, Icon::X, 18.0, false)
                        .on_hover_text("Close")
                        .clicked();
                });
            });
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 220.0).max(120.0))
                .show(ui, |ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::hover());
                    let cover = Rect::from_min_size(rect.min, vec2(64.0, 64.0));
                    art(app, ui, cover, row.artwork.as_deref());
                    let text = Rect::from_min_size(
                        rect.min + vec2(80.0, 2.0),
                        vec2((rect.width() - 80.0).max(20.0), 24.0),
                    );
                    clipped(ui, &row.name, text, theme::semibold(18.0), p.text);
                    clipped(
                        ui,
                        &row.subtitle,
                        text.translate(vec2(0.0, 24.0)),
                        theme::regular(12.0),
                        p.secondary,
                    );
                    clipped(
                        ui,
                        &row.album,
                        text.translate(vec2(0.0, 43.0)),
                        theme::regular(11.0),
                        p.dim,
                    );
                    ui.add_space(16.0);
                    columns(ui, 3, |cols| {
                        metric(&mut cols[0], &p, "Listened", &clock(row.listen_ms));
                        metric(&mut cols[1], &p, "Plays", &row.plays.to_string());
                        metric(&mut cols[2], &p, "Skips", &row.skips.to_string());
                    });
                    ui.add_space(16.0);
                    detail_pairs(
                        ui,
                        &p,
                        &[
                            ("First heard", full_date(row.first)),
                            ("Last heard", full_date(row.last)),
                        ],
                    );
                    ui.collapsing(
                        RichText::new("Playback details")
                            .font(theme::medium(12.0))
                            .color(p.secondary),
                        |ui| {
                            detail_pairs(
                                ui,
                                &p,
                                &[
                                    ("Attempts", row.attempts.to_string()),
                                    ("Completed", row.completed.to_string()),
                                    (
                                        "Unique coverage",
                                        if row.known_duration > 0 {
                                            format!(
                                                "{:.0}%",
                                                100.0 * row.coverage / row.known_duration as f64
                                            )
                                        } else {
                                            "Unknown".into()
                                        },
                                    ),
                                    ("Media time", clock(row.media_ms)),
                                ],
                            );
                        },
                    );
                    ui.add_space(20.0);
                    small(ui, &p, "Genres");
                    widgets::text_input(
                        ui,
                        &p,
                        &mut app.listening_view.genres,
                        "Hip-hop, soul…",
                        width,
                    );
                    ui.add_space(10.0);
                    small(ui, &p, "Tags & moods");
                    widgets::text_input(
                        ui,
                        &p,
                        &mut app.listening_view.tags,
                        "Late night, focus…",
                        width,
                    );
                    ui.add_space(12.0);
                    ui.horizontal_wrapped(|ui| {
                        small(ui, &p, "Rating");
                        for rating in 0..=5 {
                            let label = if rating == 0 {
                                "—".into()
                            } else {
                                rating.to_string()
                            };
                            if widgets::chip(ui, &p, &label, app.listening_view.rating == rating)
                                .on_hover_text(if rating == 0 {
                                    "Unrated".into()
                                } else {
                                    format!("{rating} out of 5")
                                })
                                .clicked()
                            {
                                app.listening_view.rating = rating;
                            }
                        }
                    });
                });
            ui.add_space(20.0);
            ui.horizontal(|ui| {
                small(ui, &p, "Only saved in Moosik");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    save = widgets::pill_button(ui, &p, "Save").clicked();
                });
            });
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
    }
    if close || save || modal.should_close() {
        app.listening_view.selected = None;
    }
}
fn history(app: &mut App, ui: &mut egui::Ui, r: &Report) {
    let p = app.palette;
    small(
        ui,
        &p,
        format!("{} · newest first", count(r.recent.len(), "listen")),
    );
    ui.add_space(12.0);
    crate::ui::virtual_rows(ui, theme::ROW_HEIGHT, r.recent.len(), |ui, i| {
        let row = &r.recent[i];
        let track = row
            .metadata
            .as_ref()
            .map(listening::Metadata::to_track)
            .or_else(|| {
                r.tracks
                    .iter()
                    .find(|t| t.id == row.track_id)
                    .map(row_track)
            })
            .unwrap_or_else(|| {
                listening::Metadata {
                    id: row.track_id.clone(),
                    title: row.title.clone(),
                    artists: vec![(row.artist.clone(), None)],
                    duration: row.duration,
                    artwork: row.artwork.clone(),
                    ..Default::default()
                }
                .to_track()
            });
        let (rect, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), theme::ROW_HEIGHT),
            Sense::hover(),
        );
        if !ui.is_rect_visible(rect) {
            return;
        }
        let wide = rect.width() > 620.0;
        let reserve = if wide { 250.0 } else { 100.0 };
        let cell = Rect::from_min_max(rect.min, pos2(rect.right() - reserve - 32.0, rect.bottom()));
        regular_song(app, ui, &track, cell, i, SongRow::Unnumbered);
        let right = rect.right() - 44.0;
        if wide {
            value_at(ui, &p, rect, right - 150.0, &clock(row.listen_ms));
            value_at(ui, &p, rect, right, &local_date(row.at));
        } else {
            value_at(ui, &p, rect, right, &clock(row.listen_ms));
        }

        let context: listening::Context = serde_json::from_str(&row.context).unwrap_or_default();
        let tooltip = format!(
            "{}\n{} listened · {:.0}% coverage\n{} · {}\n{} · Repeat {}",
            local_date(row.at),
            clock(row.listen_ms),
            row.coverage * 100.0,
            source_label(&row.source),
            outcome_label(&row.outcome),
            if context.shuffle {
                "Shuffle"
            } else {
                "In order"
            },
            context.repeat
        );
        if details_button(ui, &p, rect, i, &tooltip).clicked()
            && let Some(track) = r.tracks.iter().find(|t| t.id == row.track_id)
        {
            select_song(app, track);
        }
    });
    if r.recent.len() == 1000 {
        ui.add_space(12.0);
        small(
            ui,
            &p,
            "Showing the latest 1,000. Export all history from the ••• menu.",
        );
    }
}
fn source_label(s: &str) -> &str {
    match s {
        "youtube" => "YouTube Music",
        "spotify" => "Spotify",
        "downloaded" => "Downloaded YouTube Music",
        "local" => "Local files",
        _ => s,
    }
}
fn outcome_label(s: &str) -> &str {
    match s {
        "active" => "Listening",
        "ended" => "Ended",
        "app_closed" => "App closed",
        "replaced" | "skip" => "Changed track",
        "failed" => "Playback failed",
        "interrupted" => "Interrupted",
        _ => s,
    }
}
fn recap(app: &mut App, ui: &mut egui::Ui, r: &Report) {
    let p = app.palette;
    let compare_providers = relevant_providers(app, Some(r)).len() > 1;
    egui::Frame::new()
        .fill(p.glass_state(p.surface, 0.4))
        .corner_radius(theme::ART_RADIUS_MAX)
        .inner_margin(24.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            small(
                ui,
                &p,
                format!("MY RECAP / {}", r.query.label.to_uppercase()),
            );
            ui.add_space(16.0);
            if ui.available_width() > 650.0 {
                columns(ui, 2, |cols| {
                    recap_numbers(&mut cols[0], &p, r, compare_providers);
                    recap_art(app, &mut cols[1], r);
                });
            } else {
                recap_art(app, ui, r);
                ui.add_space(16.0);
                recap_numbers(ui, &p, r, compare_providers);
            }
            ui.add_space(20.0);
            columns(ui, 3, |cols| {
                metric(&mut cols[0], &p, "Plays", &r.plays.to_string());
                metric(&mut cols[1], &p, "Artists", &r.artists.len().to_string());
                metric(
                    &mut cols[2],
                    &p,
                    "Days listening",
                    &r.active_days.to_string(),
                );
            });
        });
    ui.add_space(16.0);
    ui.horizontal_wrapped(|ui| {
        if widgets::pill_button(ui, &p, "Share recap").clicked() {
            app.listening_view.share_feedback = None;
            app.listening_view.share = true;
        }
        if widgets::outline_button(ui, &p, "Export Markdown").clicked() {
            export_file(app, Export::Recap);
        }
    });
    if app.listening_view.share {
        share_recap(app, ui.ctx(), r);
    }
    if ui.available_width() > 720.0 {
        columns(ui, 2, |cols| {
            top_songs(app, &mut cols[0], r);
            top_artists(app, &mut cols[1], r);
        });
    } else {
        top_songs(app, ui, r);
        top_artists(app, ui, r);
    }
    section(ui, &p, "Your milestones", None);
    detail_pairs(
        ui,
        &p,
        &[
            ("New songs", r.new_tracks.to_string()),
            ("Longest streak", count(r.longest_streak, "day")),
        ],
    );
    if let Some((day, time)) = r.days.iter().max_by(|a, b| a.1.total_cmp(b.1)) {
        detail_pairs(
            ui,
            &p,
            &[("Biggest listening day", format!("{day} · {}", clock(*time)))],
        );
    }
}
fn recap_numbers(ui: &mut egui::Ui, p: &theme::Palette, r: &Report, compare_providers: bool) {
    ui.label(
        RichText::new(clock(r.listen_ms))
            .font(theme::bold(42.0))
            .color(p.text),
    );
    small(ui, p, "with your music");
    if compare_providers {
        most_used_provider(ui, p, r);
    }
    ui.add_space(20.0);
    if let Some(a) = r.artists.first() {
        small(ui, p, "Your top artist");
        ui.add(egui::Label::new(RichText::new(&a.name).font(theme::bold(22.0))).truncate());
    }
    ui.add_space(12.0);
    if let Some(t) = r.tracks.iter().find(|t| t.listen_ms > 0.0) {
        small(ui, p, "Your top song");
        ui.add(egui::Label::new(RichText::new(&t.name).font(theme::semibold(18.0))).truncate());
    }
}
fn recap_art(app: &mut App, ui: &mut egui::Ui, r: &Report) {
    let rows: Vec<_> = r
        .tracks
        .iter()
        .filter(|t| t.listen_ms > 0.0)
        .take(4)
        .collect();
    let size = ui.available_width().min(230.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), size), Sense::hover());
    let square = Rect::from_center_size(rect.center(), vec2(size, size));
    if rows.len() < 4 {
        art(
            app,
            ui,
            square,
            rows.first().and_then(|t| t.artwork.as_deref()),
        );
    } else {
        let s = (size - 8.0) / 2.0;
        for (i, t) in rows.iter().enumerate() {
            let cell = Rect::from_min_size(
                square.min + vec2((i % 2) as f32 * (s + 8.0), (i / 2) as f32 * (s + 8.0)),
                vec2(s, s),
            );
            art(app, ui, cell, t.artwork.as_deref());
        }
    }
}
fn help(app: &mut App, ctx: &egui::Context) {
    if !app.listening_view.help {
        return;
    }
    let p = app.palette;
    let mut close = false;
    let modal=egui::Modal::new(egui::Id::new("stats-help")).frame(modal_frame(&p)).backdrop_color(egui::Color32::from_black_alpha(150)).show(ctx,|ui|{
        ui.set_width((ctx.content_rect().width()-80.0).clamp(240.0,430.0));
        ui.horizontal(|ui|{ui.label(RichText::new("About your stats").font(theme::semibold(18.0)));ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{close=widgets::icon_button(ui,&p,Icon::X,18.0,false).clicked();});});
        egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-170.0).max(180.0)).show(ui,|ui|{
            for(title,body)in[
                ("Listening time","Observed playback in Moosik on this Mac, excluding pauses, buffering, seeks and app mute. System mute and whether you are paying attention cannot be detected."),
                ("Plays & completion","A play is 30 seconds or half the song, whichever is shorter. Finished means an ended session with at least 90% unique coverage. Replaying one section doesn't inflate coverage."),
                ("Dates & rankings","Time is clipped to your date range. Plays qualify inside that range; skips and completions use the session end. Artist and genre totals can overlap when a song has multiple credits or tags."),
                ("Genres & labels","Open a song to edit genres, tags and ratings. Comma-separated genres apply across its history. Untagged music stays Unknown."),
                ("Your data","Recorded locally from when tracking began. Nothing here is published to YouTube. All raw history is retained. Use the ••• menu for CSV, JSON, recap or database exports; JSON and database exports include all dates."),
                ("Recording details","Samples are saved about every five seconds. A crash can lose the last unsaved seconds. Long observation gaps are excluded. The database uses your Mac account's protection; it is not separately encrypted or automatically backed up.")
            ]{ui.add_space(14.0);ui.label(RichText::new(title).font(theme::semibold(13.0)).color(p.text));ui.label(RichText::new(body).font(theme::regular(12.0)).color(p.secondary));}
            if let Some(r)=&app.listening.report{ui.add_space(16.0);small(ui,&p,format!("{} events · {:.1} MB",r.total_events,r.database_bytes as f64/1_000_000.0));}
        });
    });
    if close || modal.should_close() {
        app.listening_view.help = false;
    }
}
fn local_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&Local).format("%b %-d, %H:%M").to_string())
        .unwrap_or_default()
}

fn full_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| {
            d.with_timezone(&Local)
                .format("%b %-d, %Y · %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// Only the opaque, previewed card is copied. Carry the crop in the capture request so
/// resizing or moving the window before the asynchronous pixel reply cannot change it.
struct CardCapture {
    rect: Rect,
    scale: f32,
    screen: egui::Vec2,
}

fn share_recap(app: &mut App, ctx: &egui::Context, r: &Report) {
    let p = app.palette;
    let copied = ctx.input(|i| {
        i.events.iter().find_map(|e| {
            let egui::Event::Screenshot {
                user_data, image, ..
            } = e
            else {
                return None;
            };
            let crop = user_data.data.as_ref()?.downcast_ref::<CardCapture>()?;
            let bounds = Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(image.width() as f32, image.height() as f32) / crop.scale,
            );
            if (bounds.size() - crop.screen).length() > 2.0 || !bounds.contains_rect(crop.rect) {
                return Some(None);
            }
            Some(Some(image.region(&crop.rect, Some(crop.scale))))
        })
    });
    if let Some(image) = copied {
        if let Some(image) = image {
            ctx.copy_image(image);
            app.listening_view.share_feedback = Some("Image copied");
        } else {
            app.listening_view.share_feedback = Some("Window resized. Try again.");
        }
    }
    let rows: Vec<_> = r
        .tracks
        .iter()
        .filter(|row| row.listen_ms > 0.0)
        .take(5)
        .collect();
    let links: Vec<_> = rows
        .iter()
        .filter(|row| moosik_resolve::VideoId::new(&row.id).is_ok())
        .map(|row| {
            format!(
                "{} — {}\nhttps://moosik.dev/#play/{}",
                row.name, row.subtitle, row.id
            )
        })
        .collect();
    let mut close = false;
    let modal = egui::Modal::new("share-listening-card".into())
        .frame(modal_frame(&p))
        .show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 70.0).clamp(190.0, 365.0));
            let compact = ctx.content_rect().height() < 540.0;
            let row_h = if compact { 31.0 } else { 42.0 };
            let card = egui::Frame::new()
                .fill(p.panel)
                .inner_margin(16)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Moosik")
                                .font(theme::bold(15.0))
                                .color(p.accent_text()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&r.query.label)
                                        .font(theme::medium(12.0))
                                        .color(p.secondary),
                                )
                                .truncate(),
                            );
                        });
                    });
                    ui.add_space(if compact { 4.0 } else { 12.0 });
                    ui.label(
                        RichText::new(clock(r.listen_ms))
                            .font(theme::bold(34.0))
                            .color(p.text),
                    );
                    small(
                        ui,
                        &p,
                        format!("listening · {}", count(r.new_tracks, "new song")),
                    );
                    if let Some(artist) = &r.discovered_artist {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("Discovered {artist}"))
                                    .font(theme::medium(13.0))
                                    .color(p.accent_text()),
                            )
                            .truncate(),
                        );
                    }
                    ui.add_space(10.0);
                    for (i, row) in rows.iter().enumerate() {
                        let (rect, _) = ui
                            .allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                        let size = row_h - 3.0;
                        let cover = Rect::from_min_size(rect.min, vec2(size, size));
                        art(app, ui, cover, row.artwork.as_deref());
                        let x = cover.right() + 10.0;
                        let line = Rect::from_min_max(
                            pos2(x, rect.top()),
                            pos2(rect.right() - 18.0, rect.center().y),
                        );
                        clipped(ui, &row.name, line, theme::semibold(14.0), p.text);
                        clipped(
                            ui,
                            &row.subtitle,
                            line.translate(vec2(0.0, row_h / 2.0)),
                            theme::regular(12.0),
                            p.secondary,
                        );
                        ui.painter().text(
                            rect.right_center(),
                            Align2::RIGHT_CENTER,
                            (i + 1).to_string(),
                            theme::medium(12.0),
                            p.dim,
                        );
                    }
                })
                .response
                .rect;
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                if widgets::pill_button(ui, &p, "Copy image").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                        CardCapture {
                            rect: card,
                            scale: ctx.pixels_per_point(),
                            screen: ctx.viewport_rect().size(),
                        },
                    )));
                }
                if !links.is_empty() && widgets::outline_button(ui, &p, "Copy song links").clicked()
                {
                    ctx.copy_text(links.join("\n\n"));
                    app.listening_view.share_feedback = Some("Song links copied");
                }
                if widgets::icon_button(ui, &p, Icon::X, 16.0, false)
                    .on_hover_text("Close")
                    .clicked()
                {
                    close = true;
                }
            });
            if let Some(message) = app.listening_view.share_feedback {
                small(ui, &p, message);
            }
        });
    if close || modal.should_close() {
        app.listening_view.share = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranking_drilldowns_use_exact_artist_ids_and_normalized_genres() {
        let row = listening::Row {
            artist_keys: vec!["artist-12".into()],
            album_key: "album-1".into(),
            genres: "Hip-Hop, Soul".into(),
            ..Default::default()
        };
        assert!(group_matches(&row, Tab::Artists, "artist-12"));
        assert!(!group_matches(&row, Tab::Artists, "artist-1"));
        assert!(group_matches(&row, Tab::Albums, "album-1"));
        assert!(group_matches(&row, Tab::Genres, "hip-hop"));
        assert!(group_matches(&row, Tab::Genres, "soul"));
        assert!(!group_matches(&row, Tab::Genres, "unknown"));
        assert!(group_matches(
            &listening::Row::default(),
            Tab::Genres,
            "unknown"
        ));
    }
}
