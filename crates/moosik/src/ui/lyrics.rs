use crate::app::App;
use crate::theme;
use crate::widgets;

use super::fullscreen::line_alpha;

pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;
    let card = widgets::card_rect(ui.available_rect_before_wrap(), theme::CARD_INSET);
    let _edge = widgets::CardEdge::new(ui, card, &p);
    widgets::clip_card_content(ui, card, theme::CARD_INSET_X, &p);
    ui.set_min_width(ui.available_width());

    let inset = theme::PAGE_INSET.left - theme::CARD_INSET.left;
    let size = egui::remap_clamp(
        card.width(),
        theme::UPNEXT_MIN..=theme::UPNEXT_MAX,
        theme::SECTION_PT..=24.0,
    )
    .round();
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(inset, 0))
        .show(ui, |ui| contents(app, ui, size));
    widgets::paint_corner_notches(ui, card, theme::CARD_INSET_X, &p);
}

fn contents(app: &mut App, ui: &mut egui::Ui, size: f32) {
    let p = app.palette;
    ui.add_space(theme::HEADER_TOP_PAD);
    ui.label(
        egui::RichText::new("Lyrics")
            .font(theme::semibold(theme::ROW_TITLE_PT))
            .color(p.secondary),
    );
    ui.add_space(theme::ROW_NUMBER_GAP);

    if app.lyrics_loading {
        widgets::contain(ui, ui.available_rect_before_wrap(), |ui| {
            widgets::skeleton_lines(ui, &p, 8);
        });
        return;
    }
    let Some(lyrics) = app
        .lyrics
        .as_ref()
        .filter(|lyrics| !lyrics.lines.is_empty())
    else {
        let message = app.lyrics_error.as_deref().unwrap_or_else(|| {
            if app.lyrics.is_some() {
                "No lyrics available"
            } else {
                "Play a song to see its lyrics"
            }
        });
        super::centered(app, ui, message);
        return;
    };

    let synced = lyrics.synced;
    let position_ms = (app.player_state.position * 1000.0) as u64;
    let active = synced
        .then(|| lyrics.lines.iter().rposition(|l| l.at_ms <= position_ms))
        .flatten();

    let reset = std::mem::take(&mut app.lyrics_scroll_reset);
    let follow = reset || active != app.lyrics_active;
    app.lyrics_active = active;

    let mut area = egui::ScrollArea::vertical()
        .id_salt("lyrics")
        .auto_shrink([false, false]);
    if reset {
        area = area.vertical_scroll_offset(0.0);
    }

    let instant = reset || widgets::reduced_motion();
    let scroll_animation = if instant {
        egui::style::ScrollAnimation::none()
    } else {
        ui.style().scroll_animation
    };
    let alpha_time = if instant { 0.0 } else { 0.28 };
    let font = theme::semibold(if synced { size } else { size * 0.85 });
    let gap = if synced { size * 0.65 } else { size * 0.45 };
    let from = widgets::painted_so_far(ui);
    let output = area.show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut clicked = None;
        ui.add_space(theme::GUTTER);
        for (i, line) in lyrics.lines.iter().enumerate() {
            let text = if line.text.is_empty() {
                "♪"
            } else {
                &line.text
            };
            let galley = ui.painter().layout(
                text.to_owned(),
                font.clone(),
                egui::Color32::PLACEHOLDER,
                ui.available_width(),
            );
            let sense = if synced {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            };
            let height = galley.size().y;
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), height + gap), sense);
            if (Some(i) == active && follow)
                || (response.gained_focus() && !ui.clip_rect().contains_rect(rect))
            {
                let line_rect =
                    egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), height));
                ui.scroll_to_rect_animation(line_rect, Some(egui::Align::Center), scroll_animation);
            }
            if synced && response.clicked() {
                clicked = Some(line.at_ms);
            }
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let alpha = if synced {
                let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), text)
                });
                let mut alpha = line_alpha(i, active);
                if response.hovered() || response.has_focus() {
                    alpha = alpha.max(0.85);
                }
                ui.ctx()
                    .animate_value_with_time(response.id.with("alpha"), alpha, alpha_time)
            } else {
                0.85
            };
            ui.painter()
                .galley(rect.min, galley, p.text.gamma_multiply(alpha));
        }
        ui.add_space(theme::GUTTER);
        clicked
    });

    if widgets::scroll_fade(&app.settings) > 0.0 {
        let band = (size * 3.0).min(output.inner_rect.height() * 0.28);
        widgets::fade_scroll_edges(ui, from, &output, band);
    }
    if let Some(at_ms) = output.inner {
        app.seek_to(at_ms as f64 / 1000.0);
    }
}
