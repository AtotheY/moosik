//! New releases, behind Explore's first button: new albums & singles.
//!
//! The same shelf shape as every other feed, so shelves render through the carousel —
//! sideways scrolling is what makes this page the reference's, not the wrapped grids
//! everywhere else uses.

use crate::app::App;
use crate::ui::{failed, shelf_carousel};
use crate::widgets;

/// The page's title, drawn by each state so it scrolls away with the list rather than sticking.
fn title(app: &App, ui: &mut egui::Ui) {
    crate::ui::page_title(ui, &app.palette, "New releases");
}

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    if app.releases_loading && app.releases.is_none() {
        // Albums & singles, then the music videos' wide cards — card shelves, never a
        // track-list one, unlike Home/Explore.
        title(app, ui);
        widgets::skeleton_shelf(
            ui,
            &app.palette,
            widgets::SkeletonShelfShape::Cards(widgets::ArtShape::Square),
            4,
        );
        widgets::skeleton_shelf(
            ui,
            &app.palette,
            widgets::SkeletonShelfShape::Cards(widgets::ArtShape::Wide),
            4,
        );
        return;
    }
    if let Some(err) = app.releases_error.clone() {
        title(app, ui);
        failed(app, ui, err, App::retry_route);
        return;
    }

    crate::ui::page_scroll(app, "releases").show(ui, |ui| {
        title(app, ui);
        // One snapshot for the frame: the carousel needs `&mut App`, which rules out
        // borrowing the feed out of `app` across the call.
        let Some(shelves) = app.releases.clone() else {
            return;
        };
        for s in shelves.iter() {
            shelf_carousel(
                app,
                ui,
                "releases",
                s,
                None,
                crate::ui::MoreButton::Pill("More"),
            );
        }
    });
}
