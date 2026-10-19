//! The search results page: two tiers, matching [`SearchScope`]'s own doc comment.
//!
//! `All` renders the one ranked cross-type response as a single list — the unfiltered response
//! ranks every type against every other, and splitting it up throws that away, which is how
//! `travis` used to surface the Scottish band ahead of Travis Scott. Every other scope instead
//! renders `app.scoped_results`, a dedicated per-category request: the mixed response often
//! carries far fewer of a given kind than actually exist (confirmed live — twelve real artist
//! matches for "the weeknd" against one in the mixed response), so narrowing to a scope re-asks
//! YouTube for that category specifically rather than filtering the same short list down further.
//!
//! There is no separate "top result" card in the `All` view. Every row is the same height, and
//! the top result is simply the first one — see [`crate::innertube::parse_search`] for why
//! YouTube's own card is folded into the ranking rather than shown separately.

use crate::app::{App, Route};
use crate::model::{Entry, SearchScope};
use crate::ui::{ListClick, centered, spec_rows};
use crate::widgets;

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    let heading = if app.searching {
        format!("Searching “{}”…", app.searched)
    } else if app.searched.is_empty() {
        "Search YouTube Music".to_owned()
    } else {
        format!("Results for “{}”", app.searched)
    };
    crate::ui::page_title(ui, &app.palette, &heading);

    if let Some(status) = app.status.clone() {
        if app.search_failed {
            // The raw chain is in the log; the page says it plainly, and no connection reads
            // as the offline page.
            let message = if crate::api::looks_offline(&status) {
                status
            } else {
                "Couldn't search right now.".to_owned()
            };
            crate::ui::failed(app, ui, message, |app| {
                let query = app.searched.clone();
                app.search_again(query);
            });
        } else {
            no_results(app, ui, &status);
        }
        return;
    }
    if app.results.is_empty() {
        if app.searching {
            // The chips are always these, so they show from the start and the rows below land
            // where the results will; a click waits for the results.
            scope_chips(app, ui, false);
            widgets::skeleton_rows(ui, &app.palette, 12);
        } else {
            centered(app, ui, "Type in the search box above. No sign-in needed.");
        }
        return;
    }

    scope_chips(app, ui, true);
    if app.search_scope == SearchScope::All {
        did_you_mean(app, ui);
    }

    if app.search_scope == SearchScope::All {
        if !app
            .results
            .items
            .iter()
            .any(|e| app.search_scope.matches(e))
        {
            centered(app, ui, "Nothing of that kind in these results.");
            return;
        }
    } else if app.scoped_loading {
        widgets::skeleton_rows(ui, &app.palette, 12);
        return;
    } else if let Some(error) = app.scoped_error.clone() {
        // The raw chain (a JSON path into YouTube's reply) is for the log, not the page; the
        // offline check still gets it, so no connection reads as the offline page.
        let message = if crate::api::looks_offline(&error) {
            error
        } else {
            format!(
                "Couldn't load {} for this search.",
                app.search_scope.label().to_lowercase()
            )
        };
        crate::ui::failed(app, ui, message, App::retry_scoped_search);
        return;
    } else if app.scoped_results.is_empty() {
        centered(app, ui, "Nothing of that kind found.");
        return;
    }

    crate::ui::page_scroll(app, "search").show(ui, |ui| {
        results_list(app, ui);
    });
}

/// YouTube's empty answer, in its own words, with its "Did you mean" spelling under it.
fn no_results(app: &mut App, ui: &mut egui::Ui, message: &str) {
    ui.add_space((ui.available_height() * 0.3).max(24.0));
    ui.vertical_centered(|ui| {
        ui.set_max_width(ui.available_width().min(460.0));
        ui.label(
            egui::RichText::new(message)
                .font(crate::theme::regular(13.0))
                .color(app.palette.dim),
        );
        if app.results.did_you_mean.is_some() {
            ui.add_space(8.0);
            // A horizontal row does not centre itself, so it is placed by last frame's width.
            let id = ui.id().with("did-you-mean-width");
            let width: f32 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(0.0);
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                let start = ui.cursor().min.x;
                let end = did_you_mean_line(app, ui);
                ui.ctx().data_mut(|d| d.insert_temp(id, end - start));
            });
        }
    });
}

/// "Did you mean: …" above results YouTube matched loosely.
fn did_you_mean(app: &mut App, ui: &mut egui::Ui) {
    if app.results.did_you_mean.is_none() {
        return;
    }
    ui.horizontal(|ui| {
        did_you_mean_line(app, ui);
    });
    ui.add_space(6.0);
}

/// The line itself, its suggestion a link that searches it. Returns where the line ends.
fn did_you_mean_line(app: &mut App, ui: &mut egui::Ui) -> f32 {
    let Some(suggestion) = app.results.did_you_mean.clone() else {
        return ui.cursor().min.x;
    };
    let p = app.palette;
    ui.spacing_mut().item_spacing.x = 4.0;
    ui.label(
        egui::RichText::new("Did you mean:")
            .font(crate::theme::regular(13.0))
            .color(p.secondary),
    );
    let link = ui.add(
        egui::Label::new(
            egui::RichText::new(&suggestion)
                .font(crate::theme::bold(13.0))
                .color(p.text),
        )
        .sense(egui::Sense::click()),
    );
    if link.hovered() {
        ui.painter().hline(
            link.rect.x_range(),
            link.rect.bottom() - 1.0,
            egui::Stroke::new(1.0, p.text),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if link.clicked() {
        app.search_again(suggestion);
    }
    link.rect.right()
}

/// Picks which tier of the page renders: `All` narrows the same ranked list down to one kind
/// (never a second request); every other scope triggers (or reuses) a dedicated per-category
/// fetch — see [`SearchScope`]'s own doc comment for why. Chips rather than a dropdown, matching
/// the rail's own filter row (`library.rs`) — a handful of mutually-exclusive choices read
/// fastest as a row you can see all of at once.
/// The scope chips. `live` is false while the first results are still coming: the chips show,
/// but a scope cannot be picked until there is a search to scope.
fn scope_chips(app: &mut App, ui: &mut egui::Ui, live: bool) {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        for scope in SearchScope::ALL {
            if widgets::chip(ui, &app.palette, scope.label(), app.search_scope == scope).clicked() {
                picked = Some(scope);
            }
        }
    });
    if let Some(scope) = picked.filter(|_| live) {
        app.search_scope = scope;
        app.fetch_scoped_search(scope);
    }
    ui.add_space(4.0);
}

/// The list. Every kind renders through the same row, so art, hover, truncation and the
/// active-track highlight are shared, and the list virtualises like every other.
///
/// `indices` is `All`'s scope filter: positions in `app.results.items` that match
/// `app.search_scope`, in the same (already-correct) order YouTube ranked them — `i` below is
/// always a position in `indices`, not in the underlying list itself. Every other scope has
/// nothing to filter (the dedicated fetch already returns only that kind), so `indices` is just
/// `0..app.scoped_results.len()`.
fn results_list(app: &mut App, ui: &mut egui::Ui) {
    let signed_in = app.signed_in();
    let scope = app.search_scope;
    let all = scope == SearchScope::All;
    let indices: Vec<usize> = if all {
        // Cached against `results_generation`: `results.items` doesn't change between one
        // search landing and the next, so re-filtering it every frame is wasted work.
        let generation = app.results_generation;
        match &app.search_all_indices {
            Some((cached_gen, indices)) if *cached_gen == generation => indices.clone(),
            _ => {
                let indices: Vec<usize> = app
                    .results
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| scope.matches(e))
                    .map(|(i, _)| i)
                    .collect();
                app.search_all_indices = Some((generation, indices.clone()));
                indices
            }
        }
    } else {
        (0..app.scoped_results.len()).collect()
    };
    let clicked = {
        let App {
            palette,
            images,
            backend,
            stats,
            liked,
            now,
            results,
            scoped_results,
            library,
            ..
        } = app;
        let items: &[Entry] = if all { &results.items } else { scoped_results };
        spec_rows(
            ui,
            palette,
            images,
            backend,
            stats,
            liked,
            indices.len(),
            |i| crate::ui::entry_row_spec(&items[indices[i]], now, library, signed_in),
        )
    };

    // Forwarded before the click: a menu choice arrives with no row click, and the `None`
    // arm below returns early.
    if let Some(action) = clicked.action {
        app.row_action(action);
    }
    let i = match clicked.click {
        // Translated back to a real position in the source list — `i` from `spec_rows` is only
        // ever a position in this frame's `indices`.
        Some(ListClick::Row(i)) => indices[i],
        Some(ListClick::Link(route)) => {
            app.navigate(route);
            return;
        }
        None => return,
    };
    let item = if all {
        &app.results.items[i]
    } else {
        &app.scoped_results[i]
    };
    match item {
        Entry::Song(_) | Entry::Video(_) => {
            let (index, queue) = if all {
                (app.results.playable_index(i), app.results.playable())
            } else {
                (
                    crate::ui::playable_index(&app.scoped_results, i),
                    crate::ui::playable_queue(&app.scoped_results),
                )
            };
            if let Some(index) = index {
                app.play_row(queue, index);
            }
        }
        Entry::Artist(a) => {
            let id = a.id.clone();
            app.navigate(Route::Artist(id));
        }
        Entry::Album(a) => {
            let id = a.id.clone();
            app.navigate(Route::Album(id));
        }
        Entry::Playlist(p) => {
            let id = p.id.clone();
            app.navigate(Route::Playlist(id));
        }
        Entry::Mood(m) => {
            let params = m.params.clone();
            app.navigate(Route::Mood(params));
        }
    }
}
