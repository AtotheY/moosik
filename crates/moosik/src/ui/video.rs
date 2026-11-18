//! The music-video view: a black content card with the software-rendered frame.
//!
//! The frame is rendered at the video's own size — the software renderer scales on the CPU,
//! single-threaded, so letting the GPU fit it into the card is the cheap direction — and
//! centred inside the card with its own aspect.

use crate::app::App;
use crate::player::Player;
use crate::theme;

/// The longest side of the frame buffer, in physical pixels. The 720p selector already meets
/// it; this bounds anything unusual that slips through.
const MAX_PX: f32 = 1280.0;

pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let full = app.fs.open && app.fs.cinema;
    let radius = egui::CornerRadius::same(if full { 0 } else { theme::PANEL_RADIUS });
    ui.painter().rect_filled(rect, radius, egui::Color32::BLACK);
    paint_frame(app, ui, rect, radius);
    let surface = ui.interact(rect, ui.id().with("video-surface"), egui::Sense::click());
    if ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_pressed()) {
        app.fs.touch();
    }
    let remaining = app.fs.remaining_controls_time();
    let menu_id = ui.id().with("video-captions");
    let menu_open = egui::Popup::is_id_open(ui.ctx(), menu_id);
    let show = !full || !remaining.is_zero() || menu_open;
    let (toggle, controls_used) = if show {
        if full && !remaining.is_zero() {
            ui.ctx().request_repaint_after(remaining);
        }
        controls(app, ui, rect, full, menu_id, menu_open)
    } else {
        (false, false)
    };
    // A double-click changes only the local window. Wait until the second click has
    // been ruled out before sending a single click to the host's transport.
    let track_id = app.now.track.as_ref().map(|t| t.video_id.as_str());
    let key = surface.id.with(("play-pause", track_id));
    let frame = ui.ctx().cumulative_frame_nr();
    let now = ui.input(|i| i.time);
    let pending = ui.data_mut(|d| d.remove_temp::<(f64, u64)>(key));
    if toggle || (!controls_used && surface.double_clicked()) {
        if full {
            app.fs_exit(ui.ctx());
        } else {
            app.fs_enter(ui.ctx());
        }
    } else if !controls_used && !app.following() {
        let deadline = if surface.clicked() {
            Some(now + ui.ctx().options(|o| o.input_options.max_double_click_delay))
        } else {
            pending
                .filter(|(_, seen)| frame.saturating_sub(*seen) <= 1)
                .map(|(at, _)| at)
        };
        if let Some(at) = deadline {
            if now >= at {
                app.toggle_playback();
            } else {
                ui.data_mut(|d| d.insert_temp(key, (at, frame)));
                ui.ctx().request_repaint_after_secs((at - now) as f32);
            }
        }
    }
}

/// A small local control strip, available even when fullscreen hides the player bar.
fn controls(
    app: &mut App,
    ui: &mut egui::Ui,
    rect: egui::Rect,
    full: bool,
    menu_id: egui::Id,
    menu_open: bool,
) -> (bool, bool) {
    use crate::{icons::Icon, widgets};
    let bar = egui::Rect::from_min_size(
        rect.right_bottom() - egui::vec2(254.0, 50.0),
        egui::vec2(242.0, 38.0),
    );
    ui.painter()
        .rect_filled(bar, theme::RADIUS, egui::Color32::from_black_alpha(190));
    let mut controls = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("video-controls")
            .max_rect(bar.shrink2(egui::vec2(6.0, 0.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    controls.spacing_mut().item_spacing.x = 8.0;
    // The video always has a dark backdrop, including when the rest of the app is light.
    let mut p = theme::Palette::dark();
    p.accent = app.palette.accent;
    let (icon, label) = if full {
        (Icon::Minimize2, "Exit fullscreen (Esc)")
    } else {
        (Icon::Maximize2, "Fullscreen (F)")
    };
    let toggle = widgets::icon_button(&mut controls, &p, icon, 17.0, false)
        .on_hover_text(label)
        .clicked();
    let cc = widgets::chip(&mut controls, &p, "CC", app.captions.selected.is_some()).on_hover_text(
        if app.captions.loading {
            "Loading captions…"
        } else {
            "Captions"
        },
    );
    let menu_palette = app.palette;
    let mut selection = None;
    egui::Popup::from_response(&cc)
        .id(menu_id)
        .kind(egui::PopupKind::Menu)
        .frame(widgets::menu_frame(ui, &menu_palette))
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
        .align(egui::RectAlign::TOP_END)
        .width(240.0)
        .gap(6.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .open_memory(if cc.clicked() {
            Some(egui::SetOpenCommand::Toggle)
        } else {
            None
        })
        .show(|ui| {
            let off = app.captions.selected.is_none();
            if widgets::menu_item_plain(ui, &menu_palette, "Off", off.then_some("✓")).clicked() {
                selection = Some(None);
                ui.close();
            }
            widgets::menu_divider(ui, &menu_palette);
            if app.captions.tracks.is_empty() {
                ui.label(
                    egui::RichText::new(if app.captions.ready || app.now.error.is_some() {
                        "No captions available"
                    } else {
                        "Loading languages…"
                    })
                    .font(theme::regular(12.0))
                    .color(menu_palette.dim),
                );
            } else {
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        for (i, track) in app.captions.tracks.iter().enumerate() {
                            let chosen = app.captions.selected == Some(i);
                            let mark =
                                chosen.then_some(if app.captions.loading { "…" } else { "✓" });
                            if widgets::menu_item_plain(ui, &menu_palette, &track.label, mark)
                                .clicked()
                            {
                                selection = Some(Some(i));
                                ui.close();
                            }
                        }
                    });
            }
        });
    if let Some(selected) = selection {
        app.select_caption(selected);
    }
    let volume = if app.muted { 0.0 } else { app.last_volume };
    if let Some(v) = widgets::volume_slider(&mut controls, &p, volume, app.last_volume) {
        app.set_user_volume(v);
    }
    let used = menu_open
        || cc.clicked()
        || ui.input(|i| i.pointer.hover_pos().is_some_and(|pos| bar.contains(pos)));
    if used {
        app.fs.touch();
    }
    (toggle, used)
}

/// The video frame (or the track's still while it loads) fitted inside `rect`, rounded to
/// `radius` where it reaches the corners, with the loading caption. Paints no backdrop: the
/// caller owns what is behind it.
pub(crate) fn paint_frame(
    app: &mut App,
    ui: &mut egui::Ui,
    rect: egui::Rect,
    radius: egui::CornerRadius,
) {
    // The track's wide still: the placeholder while the video resolves, and the backdrop if no
    // frame ever arrives. Cloned out of the cache so the surface below can borrow `app`.
    let url = app.now.track.as_ref().and_then(|t| t.thumbnail.clone());
    let still = super::art_texture_wide(
        &mut app.images,
        &app.backend,
        ui.ctx(),
        url.as_ref(),
        rect.width(),
    )
    .cloned();

    // Read the size before borrowing the surface: the video's own when one is loaded, the
    // card's otherwise (mpv letterboxes into it until the first frame reports a size).
    let native = app.player.as_ref().and_then(Player::video_size);
    let ppp = ui.ctx().pixels_per_point();
    let size = match native {
        Some((w, h)) => capped(w, h),
        None => {
            let width = (rect.width() * ppp).clamp(2.0, MAX_PX);
            let height = (width * (rect.height() / rect.width().max(1.0))).clamp(2.0, MAX_PX);
            (width.round() as usize, height.round() as usize)
        }
    };

    let frame = app
        .video
        .as_mut()
        .and_then(|surface| surface.frame(ui, size));
    // While the next video resolves, the previous frame is still in the texture; the still of
    // the track that is actually loading is the honest thing to show.
    let painted = if app.now.resolving {
        still.or(frame)
    } else {
        frame.or(still)
    };
    if let Some(texture) = painted {
        let fitted = fit_rect(rect, texture.size_vec2());
        egui::Image::from_texture(&texture)
            .fit_to_exact_size(fitted.size())
            // The card is already rounded; the video only rounds where it reaches the card's
            // corners. Letterboxed, with black above and below, its own corners are square.
            .corner_radius(crate::widgets::inner_radius(radius, rect, fitted))
            .paint_at(ui, fitted);
    }

    if app.now.resolving || app.now.error.is_some() {
        loading_caption(app, ui, rect);
    }
}

/// A quiet caption while the video is being fetched, low in the card so the still stays readable.
fn loading_caption(app: &App, ui: &egui::Ui, rect: egui::Rect) {
    let galley = ui.painter().layout_no_wrap(
        if app.now.error.is_some() {
            "Video unavailable"
        } else {
            "Loading video…"
        }
        .into(),
        theme::regular(12.0),
        app.palette.dim,
    );
    let pos = egui::pos2(
        rect.center().x - galley.size().x / 2.0,
        rect.bottom() - 20.0 - galley.size().y,
    );
    ui.painter().galley(pos, galley, app.palette.dim);
}

/// Scale a video size down so neither side exceeds [`MAX_PX`], keeping its aspect.
fn capped(w: usize, h: usize) -> (usize, usize) {
    let longest = w.max(h) as f32;
    if longest <= MAX_PX {
        return (w, h);
    }
    let scale = MAX_PX / longest;
    (
        ((w as f32 * scale).round() as usize).max(2),
        ((h as f32 * scale).round() as usize).max(2),
    )
}

/// The largest rect with `texture`'s aspect that fits inside `card`, centred.
fn fit_rect(card: egui::Rect, texture: egui::Vec2) -> egui::Rect {
    if texture.x <= 0.0 || texture.y <= 0.0 {
        return card;
    }
    let scale = (card.width() / texture.x).min(card.height() / texture.y);
    egui::Rect::from_center_size(card.center(), texture * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A landscape video fills the card's width and letterboxes; a portrait one fills its
    /// height. Either way the aspect is the video's, never the card's.
    #[test]
    fn the_frame_keeps_its_aspect_inside_the_card() {
        let card = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        let landscape = fit_rect(card, egui::vec2(1280.0, 720.0));
        assert_eq!(landscape.width(), 800.0);
        assert_eq!(landscape.height(), 450.0);
        assert_eq!(landscape.center(), card.center());

        let portrait = fit_rect(card, egui::vec2(720.0, 1280.0));
        assert_eq!(portrait.height(), 600.0);
        assert!(portrait.width() < 400.0);
        assert_eq!(portrait.center(), card.center());
    }

    #[test]
    fn a_frame_with_no_size_falls_back_to_the_card() {
        let card = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        assert_eq!(fit_rect(card, egui::Vec2::ZERO), card);
    }

    /// The render buffer never exceeds the cap on either side, whichever way the video is.
    #[test]
    fn the_frame_buffer_is_capped_on_its_longest_side() {
        assert_eq!(capped(1920, 1080), (1280, 720));
        assert_eq!(capped(2560, 1440), (1280, 720));
        // Already within the cap: untouched, portrait or not.
        assert_eq!(capped(640, 360), (640, 360));
        assert_eq!(capped(720, 1280), (720, 1280));
    }
}
