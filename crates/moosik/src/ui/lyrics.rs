//! The lyrics panel: the now-playing song's words, synced to playback when we have timings.
//!
//! Shares the right panel with the queue — opening one closes the other — so it takes the same
//! width and insets the queue uses.

use crate::app::App;
use crate::theme;
use crate::widgets;

pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;
    // The card's own rect, before the header and list consume the ui: the corner repaint at
    // the end needs its rounded edges.
    let card = widgets::card_rect(ui.available_rect_before_wrap(), theme::CARD_INSET);
    let _edge = widgets::CardEdge::new(ui, card, &p);
    widgets::clip_card_content(ui, card, theme::CARD_INSET_X, &p);

    if app.lyrics_loading {
        // The card is as wide as what it holds, and the skeleton's ragged lines reach only part
        // of the way: without this the card's fill stopped short of its right edge. The loaded
        // lyrics fill it through their scroll area.
        ui.set_min_width(ui.available_width());
        title(ui, &p);
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            ui.vertical(|ui| widgets::skeleton_lines(ui, &p, 8));
        });
        return;
    }
    let Some(lyrics) = app.lyrics.as_ref() else {
        title(ui, &p);
        let message = app
            .lyrics_error
            .clone()
            .unwrap_or_else(|| "Play a song to see its lyrics".to_owned());
        crate::ui::centered(app, ui, &message);
        return;
    };

    // Which line is playing. Only synced lyrics know; plain text has no highlight.
    let position_ms = (app.player_state.position * 1000.0) as u64;
    let active = lyrics
        .synced
        .then(|| lyrics.lines.iter().rposition(|l| l.at_ms <= position_ms))
        .flatten();

    let mut area = egui::ScrollArea::vertical()
        .id_salt("lyrics")
        .auto_shrink([false, false]);
    if app.lyrics_scroll_reset {
        area = area.vertical_scroll_offset(0.0);
        app.lyrics_scroll_reset = false;
        // Land on the top of the panel, not wherever the active line happens to be — the
        // sync below then treats this frame's line as already seen, so the scroll-to-active
        // below doesn't immediately undo the reset.
        app.lyrics_active = active;
    }
    // Only scroll when the active line actually changed since last frame — not every repaint,
    // which used to snap the view back the instant a paused listener scrolled away to read
    // ahead. Since `active` only advances as `position_ms` crosses a new line, this is also
    // exactly "only while playback advances": paused, position never changes, so neither does
    // `active`.
    let scroll = active != app.lyrics_active;
    app.lyrics_active = active;

    let synced = lyrics.synced;
    let from = widgets::painted_so_far(ui);
    let output = area.show(ui, |ui| {
        // Inside the scroll: the title scrolls away with the lyrics rather than staying
        // pinned.
        title(ui, &p);
        let mut clicked = None;
        for (i, line) in lyrics.lines.iter().enumerate() {
            // The gap goes before each line but the first, not after each: after the last
            // one it was blank space below the lyrics, inside a card whose content runs to
            // its bottom edge.
            if i > 0 {
                ui.add_space(4.0);
            }
            let is_active = Some(i) == active;
            let font = if is_active {
                theme::semibold(15.0)
            } else {
                theme::regular(14.0)
            };
            let text = if line.text.is_empty() {
                "♪"
            } else {
                &line.text
            };
            // Laid out before the colour is known: a synced line lights up under the
            // pointer, which needs the row's rect first. The whole row is the target, so a
            // short line is as easy to hit as a long one.
            let galley = ui.painter().layout(
                text.to_owned(),
                font,
                egui::Color32::PLACEHOLDER,
                ui.available_width(),
            );
            let sense = if synced {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            };
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), galley.size().y), sense);
            let hovered = synced && response.hovered();
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let color = if is_active {
                p.accent_text()
            } else if hovered {
                p.text
            } else {
                p.secondary
            };
            ui.painter().galley(rect.min, galley, color);
            if synced && response.clicked() {
                clicked = Some(line.at_ms);
            }
            if is_active && scroll {
                ui.scroll_to_rect(
                    rect.expand2(egui::vec2(0.0, 48.0)),
                    Some(egui::Align::Center),
                );
            }
        }
        clicked
    });
    // The title scrolls with the words, so at the top of the lyrics nothing fades there.
    widgets::fade_scroll_edges(ui, from, &output, widgets::scroll_fade(&app.settings));
    let clicked = output.inner;
    // A click on a synced line plays from where it starts.
    if let Some(at_ms) = clicked {
        app.seek_to(at_ms as f64 / 1000.0);
    }
    // The card's rounded corners, over everything the lyrics drew: the list runs to the card's
    // bottom edge now.
    widgets::paint_corner_notches(ui, card, theme::CARD_INSET_X, &p);
}

/// The panel's own title, a hair right of the lyrics below it by eye — the same nudge the
/// queue's own header takes.
fn title(ui: &mut egui::Ui, p: &theme::Palette) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new("Lyrics")
                .font(theme::semibold(14.0))
                .color(p.text),
        );
    });
    ui.add_space(8.0);
}
