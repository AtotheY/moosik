//! Searchable listening history and personal rediscovery, using the shared song rows.

use crate::app::App;
use crate::auth::AuthState;
use crate::theme;
use crate::ui::{centered, entry_rows, failed, section};
use crate::widgets;

fn youtube_page(app: &mut App, ui: &mut egui::Ui) {
    match &app.auth {
        AuthState::SignedIn { .. } => content(app, ui),
        AuthState::Checking => {
            widgets::skeleton_shelf(ui, &app.palette, widgets::SkeletonShelfShape::Rows, 8);
            widgets::skeleton_shelf(ui, &app.palette, widgets::SkeletonShelfShape::Rows, 4);
        }
        AuthState::Failed(_) if app.offline => {
            ui.label(
                egui::RichText::new("Your history loads once you're back online.")
                    .font(theme::regular(13.0))
                    .color(app.palette.secondary),
            );
        }
        _ => {
            ui.label(
                egui::RichText::new("Sign in to see what you played.")
                    .font(theme::regular(13.0))
                    .color(app.palette.secondary),
            );
            ui.add_space(12.0);
            if widgets::pill_button(ui, &app.palette, "Sign in").clicked() {
                app.sign_in();
            }
        }
    }
}

fn content(app: &mut App, ui: &mut egui::Ui) {
    if app.history_loading && app.history_shelves.is_none() {
        // Dated shelves (Today, Yesterday, ...) are always track lists — several titled row
        // blocks, not one flat 8-row list with no shelf boundaries.
        widgets::skeleton_shelf(ui, &app.palette, widgets::SkeletonShelfShape::Rows, 8);
        widgets::skeleton_shelf(ui, &app.palette, widgets::SkeletonShelfShape::Rows, 4);
        return;
    }
    if let Some(err) = app.history_error.clone() {
        failed(app, ui, err, App::retry_route);
        return;
    }

    // One snapshot for the frame: `shelf` needs `&mut App`, which rules out borrowing the
    // shelves out of `app` across the call.
    let Some(shelves) = app.history_shelves.clone() else {
        return;
    };
    // Legitimately empty — a fresh account, paused watch history, or just nothing played yet
    // — is not an error (see `Api::history`'s own doc comment), so it gets a plain message
    // rather than nothing at all.
    if shelves.is_empty() {
        centered(app, ui, "Nothing played yet.");
        return;
    }
    crate::ui::page_scroll(app, "history").show(ui, |ui| {
        for s in shelves.iter() {
            if s.items.is_empty() {
                continue;
            }
            section(ui, &app.palette, &s.title);
            // Each dated shelf restarts its own row-index-based widget ids at 0 — without a
            // namespace per shelf, "Today" row 0 and "Yesterday" row 0 collide (the same bug
            // already fixed once for upnext.rs's "Now playing" vs "Next up").
            ui.push_id(&s.title, |ui| entry_rows(app, ui, &s.items));
        }
    });
}

#[derive(Default)]
pub struct View {
    tab: usize,
    search: String,
    days: usize,
    discovery: usize,
}

pub(crate) fn all_time_query() -> crate::listening::Query {
    use chrono::{Local, TimeZone};
    let today = Local::now().date_naive();
    let tomorrow = today.succ_opt().unwrap_or(today);
    let to = Local
        .from_local_datetime(&tomorrow.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(crate::listening::now_ms);
    crate::listening::Query {
        from: 0,
        to,
        source: String::new(),
        label: "All time".into(),
    }
}

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;
    crate::ui::page_title(ui, &p, "Listening history");
    ui.horizontal_wrapped(|ui| {
        for (i, label) in ["Recently played", "Rediscover", "YouTube history"]
            .iter()
            .enumerate()
        {
            if widgets::chip(ui, &p, label, app.history_view.tab == i).clicked() {
                app.history_view.tab = i;
            }
        }
    });
    ui.add_space(16.0);
    if app.history_view.tab == 2 {
        youtube_page(app, ui);
        return;
    }
    let query = all_time_query();
    app.listening.request(query.clone(), false);
    let Some(report) = app
        .listening
        .report
        .as_ref()
        .filter(|r| r.query == query)
        .cloned()
    else {
        if let Some(error) = &app.listening.error {
            ui.colored_label(p.danger, error);
        } else {
            widgets::skeleton_track_rows(ui, &p, 6, false);
        }
        return;
    };
    if app.history_view.tab == 1 {
        rediscover(app, ui, &report);
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.history_view.search)
                .hint_text("Find a song or artist you heard")
                .font(theme::regular(13.0))
                .margin(egui::vec2(8.0, 7.0))
                .desired_width((ui.available_width() - 155.0).clamp(160.0, 350.0)),
        );
        let labels = ["All time", "Today", "Last 7 days", "Last 30 days"];
        if let Some(i) = widgets::text_dropdown(
            ui,
            &p,
            egui::Id::new("history-period"),
            labels[app.history_view.days],
            &labels,
        ) {
            app.history_view.days = i;
        }
    });
    ui.add_space(10.0);
    let needle = app.history_view.search.trim().to_lowercase();
    let cutoff = match app.history_view.days {
        1 => {
            use chrono::TimeZone;
            chrono::Local
                .from_local_datetime(
                    &chrono::Local::now()
                        .date_naive()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                )
                .earliest()
                .map(|d| d.timestamp_millis())
                .unwrap_or_else(crate::listening::now_ms)
        }
        2 => crate::listening::now_ms() - 7 * 86_400_000,
        3 => crate::listening::now_ms() - 30 * 86_400_000,
        _ => 0,
    };
    let rows: Vec<_> = report
        .recent
        .iter()
        .filter(|r| {
            r.listen_ms > 0.0
                && r.at >= cutoff
                && format!("{} {}", r.title, r.artist)
                    .to_lowercase()
                    .contains(&needle)
        })
        .collect();
    ui.label(
        egui::RichText::new("Played in Moosik on this Mac · newest first")
            .font(theme::regular(12.0))
            .color(p.dim),
    );
    ui.add_space(12.0);
    if rows.is_empty() {
        ui.add_space(30.0);
        ui.label(
            egui::RichText::new(if report.recent.is_empty() {
                "Your next great find starts here"
            } else {
                "No matching listens"
            })
            .font(theme::bold(theme::SECTION_PT))
            .color(p.text),
        );
        ui.label(
            egui::RichText::new(if report.recent.is_empty() {
                "Play something in Moosik. Come back to find it, replay it, or save it."
            } else {
                "Try a different song, artist, or date range."
            })
            .font(theme::regular(13.0))
            .color(p.secondary),
        );
        return;
    }
    crate::ui::page_scroll(app, "local-listening-history").show(ui, |ui| {
        // A small date label per row keeps virtual heights fixed and remains useful while scrolling.
        crate::ui::virtual_rows(ui, theme::ROW_HEIGHT + 22.0, rows.len(), |ui, i| {
            let row = rows[i];
            let date = chrono::DateTime::from_timestamp_millis(row.at).map(|d| d.with_timezone(&chrono::Local));
            let stamp = date.map(|d| {
                let today = chrono::Local::now().date_naive();
                let day = if d.date_naive() == today { "Today".into() }
                    else if Some(d.date_naive()) == today.pred_opt() { "Yesterday".into() }
                    else { d.format("%a, %b %-d").to_string() };
                format!("{day} · {}", d.format("%-I:%M %p"))
            }).unwrap_or_default();
            ui.label(egui::RichText::new(stamp).font(theme::regular(11.0)).color(p.dim));
            if let Some(meta) = &row.metadata {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), theme::ROW_HEIGHT), egui::Sense::hover());
                crate::ui::listening::regular_song(app, ui, &meta.to_track(), rect, i, super::listening::SongRow::Unnumbered);
            }
        });
        if report.recent.len() >= 5000 { ui.label("Showing the latest 5,000 listens. The full ledger is available in Listening stats → Export."); }
    });
}

fn rediscover(app: &mut App, ui: &mut egui::Ui, report: &crate::listening::Report) {
    let p = app.palette;
    let rules = crate::local_playlists::SmartRule::ALL;
    ui.horizontal_wrapped(|ui| {
        for (i, rule) in rules.iter().enumerate() {
            if widgets::chip(ui, &p, rule.title(), app.history_view.discovery == i).clicked() {
                app.history_view.discovery = i;
            }
        }
    });
    let rule = rules[app.history_view.discovery];
    ui.add_space(16.0);
    ui.label(
        egui::RichText::new(rule.description())
            .font(theme::regular(13.0))
            .color(p.secondary),
    );
    let tracks = rule.tracks(report);
    ui.add_space(12.0);
    ui.horizontal_wrapped(|ui| {
        if !tracks.is_empty() && widgets::pill_button(ui, &p, "Play").clicked() {
            app.play_row(tracks.clone(), 0);
        }
        if widgets::chip(ui, &p, "Keep as smart playlist", false).clicked() {
            app.new_playlist = Some(rule.title().into());
            app.new_playlist_kind = crate::app::NewPlaylistKind::Private {
                folder: None,
                tracks: vec![],
                rule: Some(rule),
            };
        }
    });
    ui.add_space(16.0);
    if tracks.is_empty() {
        ui.label(
            egui::RichText::new("Nothing here yet. This collection grows with your listening.")
                .font(theme::regular(13.0))
                .color(p.dim),
        );
        return;
    }
    crate::ui::page_scroll(app, "rediscover-tracks").show(ui, |ui| {
        let signed_in = app.signed_in();
        let result = crate::ui::track_rows(
            ui,
            &p,
            &mut app.images,
            &app.backend,
            &mut app.stats,
            &app.liked,
            &app.now,
            &tracks,
            false,
            None,
            &app.library.playlists.items,
            signed_in,
            false,
            false,
        );
        if let Some(action) = result.action {
            app.row_action(action);
        }
        match result.click {
            Some(crate::ui::ListClick::Row(i)) => app.play_row(tracks.clone(), i),
            Some(crate::ui::ListClick::Link(route)) => app.navigate(route),
            _ => {}
        }
    });
}
