//! Every mood category, behind Explore's third button.
//!
//! Sections of stripe chips through the same grid the Explore feed's own Moods & genres shelf
//! uses — the typed categories carry no stripe colour, so those chips get the dim outline
//! rather than an invented one.

use crate::app::{App, Route};
use crate::ui::{failed, mood_chips, section};
use crate::widgets;

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    if app.moods_loading && app.moods.is_none() {
        // No cover art anywhere on this page — colour-stripe chips, not photo cards.
        crate::ui::page_title(ui, &app.palette, "Moods & genres");
        // For you, Moods & moments, Genres: a row, two, and a longer block.
        for rows in [1, 2, 4] {
            widgets::skeleton_shelf(ui, &app.palette, widgets::SkeletonShelfShape::Chips, rows);
        }
        return;
    }
    if let Some(err) = app.moods_error.clone() {
        crate::ui::page_title(ui, &app.palette, "Moods & genres");
        failed(app, ui, err, App::retry_route);
        return;
    }

    // The title scrolls away with the page rather than staying pinned over it.
    crate::ui::page_scroll(app, "moods").show(ui, |ui| {
        crate::ui::page_title(ui, &app.palette, "Moods & genres");
        // One snapshot for the frame: the chips need `&mut App` to navigate, which rules
        // out borrowing the sections out of `app` across the call.
        let Some(sections) = app.moods.clone() else {
            return;
        };
        for sec in sections.iter() {
            section(ui, &app.palette, &sec.name);
            // The chips read the moods where they are, and the click comes back as an index
            // into that same list — no per-frame conversion into entries.
            if let Some(j) = mood_chips(
                ui,
                &app.palette,
                sec.moods.iter().map(|m| (m.title.as_str(), m.colour)),
            ) && let Some(mood) = sec.moods.get(j)
            {
                app.navigate(Route::Mood(mood.params.clone()));
            }
        }
    });
}
