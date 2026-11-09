//! Home section customization, opened from Home or Appearance settings.
//! Moving a section saves its order and rearranges the loaded feed.

use crate::app::{App, Route};
use crate::icons::Icon;
use crate::model::{HomeSection, Provider, default_order_keys, normalise_order};
use crate::settings::HomeSource;
use crate::theme;
use crate::widgets;

const ROW_HEIGHT: f32 = 44.0;
const WIDTH: f32 = 440.0;
const NARROW_WIDTH: f32 = 320.0;
/// How near the list's edge a dragged row has to be before the list scrolls toward it, and how
/// fast it goes.
const EDGE: f32 = 36.0;
const EDGE_SPEED: f32 = 9.0;

/// Why Home may not match the list, in the plainest words that are still true: YouTube decides
/// what exists and sends it in batches, and a late section goes below what is already showing.
pub(crate) const DISCLAIMER: &str = "Sources choose which feeds appear on Home. Mood filters \
apply to YouTube Music only. Providers send sections in batches; a section that arrives late \
is added below the sections already shown. Your section order applies within each provider. \
Unlisted sections use Everything else.";

/// The order after dragging row `from` onto row `to`: the row is taken out and put in `to`'s
/// place, which is what the drop line shown while dragging promises.
pub(crate) fn moved(order: &[HomeSection], from: usize, to: usize) -> Vec<HomeSection> {
    let mut out = order.to_vec();
    if from < out.len() && to < out.len() && from != to {
        let item = out.remove(from);
        out.insert(to, item);
    }
    out
}

fn panel_header(ui: &mut egui::Ui, p: &theme::Palette) -> (bool, egui::Vec2) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 15.0, rect.center().y),
        egui::Vec2::splat(30.0),
    );
    let drag_rect = rect.with_max_x(close_rect.left() - 8.0);
    let drag = ui
        .interact(
            drag_rect,
            ui.id().with("home-order-header-drag"),
            egui::Sense::drag(),
        )
        .on_hover_text("Drag to move");
    if drag.hovered() || drag.dragged() {
        ui.ctx().set_cursor_icon(if drag.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }
    Icon::Grip.image(p.secondary, 16.0).paint_at(
        ui,
        egui::Rect::from_center_size(
            rect.left_center() + egui::vec2(8.0, 0.0),
            egui::Vec2::splat(16.0),
        ),
    );
    let text_x = rect.left() + 28.0;
    crate::emoji::paint_line(
        ui,
        egui::pos2(text_x, rect.center().y),
        &widgets::truncate_to(
            ui,
            "Customize Home",
            theme::semibold(16.0),
            (drag_rect.right() - text_x).max(0.0),
        ),
        theme::semibold(16.0),
        p.text,
    );
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        egui::Stroke::new(1.0, p.outline),
    );
    let mut controls = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("home-order-close")
            .max_rect(close_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let close = widgets::icon_button(&mut controls, p, Icon::X, 16.0, false).on_hover_text("Close");
    close.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            "Close customization",
        )
    });
    (
        close.clicked(),
        if drag.dragged() {
            ui.input(|input| input.pointer.delta())
        } else {
            egui::Vec2::ZERO
        },
    )
}

pub(super) fn open(app: &mut App) {
    app.set_home_filter(None);
    app.navigate(Route::Home);
    app.home_order_open = true;
}

pub(crate) fn window(app: &mut App, ui: &mut egui::Ui) {
    if !app.home_order_open {
        return;
    }
    if app.current_route() != Route::Home {
        app.home_order_open = false;
        return;
    }
    let Some(screen) = ui.ctx().input(|i| i.raw.screen_rect) else {
        return;
    };
    let p = app.palette;
    let order = normalise_order(&app.settings.home_order);
    let available = app.available_providers();
    let mut source = app.effective_home_source();
    let mut primary = app
        .settings
        .home_primary_provider
        .filter(|provider| available.contains(provider));
    let mut source_changed = false;
    let mut primary_changed = false;
    let desired_width = if app.content_rect.width() < WIDTH + NARROW_WIDTH {
        NARROW_WIDTH
    } else {
        WIDTH
    };
    let width = desired_width.min((screen.width() - 40.0).max(0.0));
    let height = (app.content_rect.height() - theme::GUTTER * 2.0).clamp(0.0, 680.0);
    let window_id = egui::Id::new("refryt-home-customizer");
    let position_id = window_id.with("position");
    let position = ui
        .ctx()
        .data(|data| data.get_temp::<egui::Pos2>(position_id))
        .unwrap_or_else(|| {
            egui::pos2(
                screen.right() - width - theme::PAGE_INSET_X,
                app.content_rect.top() + theme::GUTTER,
            )
        });

    let mut close = false;
    let mut drag_delta = egui::Vec2::ZERO;
    let mut reorder: Option<(usize, usize)> = None;
    let mut reset = false;
    let shown = egui::Window::new("Customize Home")
        .id(window_id)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .current_pos(position)
        .fixed_size(egui::vec2(width, height))
        .constrain_to(screen.shrink(theme::GUTTER))
        .frame(
            egui::Frame::new()
                .fill(p.overlay)
                .stroke(egui::Stroke::new(1.0, p.outline))
                .corner_radius(theme::RADIUS + 2)
                .inner_margin(16),
        )
        .show(ui.ctx(), |body| {
            body.spacing_mut().item_spacing.y = 0.0;
            (close, drag_delta) = panel_header(body, &p);
            body.add_space(16.0);

            let scroll_h = (body.available_height() - 44.0).max(0.0);
            egui::ScrollArea::vertical()
                .id_salt("home-order-list")
                .max_height(scroll_h)
                .auto_shrink([false, false])
                .show(body, |list| {
                    list.spacing_mut().item_spacing.y = 0.0;
                    if available.len() > 1 {
                        list.label(
                            egui::RichText::new("Source")
                                .font(theme::semibold(13.0))
                                .color(p.text),
                        );
                        list.add_space(8.0);
                        list.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.y = theme::GUTTER;
                            for option in
                                HomeSource::ALL.into_iter().filter(|option| match option {
                                    HomeSource::Combined => true,
                                    HomeSource::YouTubeMusic => {
                                        available.contains(&Provider::YouTubeMusic)
                                    }
                                    HomeSource::Spotify => available.contains(&Provider::Spotify),
                                    HomeSource::Local => available.contains(&Provider::Local),
                                })
                            {
                                if widgets::chip(ui, &p, option.label(), source == option).clicked()
                                {
                                    source = option;
                                    source_changed = true;
                                }
                            }
                        });
                        list.add_space(20.0);
                        if source == HomeSource::Combined {
                            list.label(
                                egui::RichText::new("Priority")
                                    .font(theme::semibold(13.0))
                                    .color(p.text),
                            );
                            list.add_space(6.0);
                            list.label(
                                egui::RichText::new(
                                    "Show more sections from this provider near the top.",
                                )
                                .font(theme::regular(12.0))
                                .color(p.secondary),
                            );
                            list.add_space(8.0);
                            list.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.y = theme::GUTTER;
                                if widgets::chip(ui, &p, "Balanced", primary.is_none()).clicked() {
                                    primary = None;
                                    primary_changed = true;
                                }
                                for &provider in &available {
                                    if widgets::chip(
                                        ui,
                                        &p,
                                        provider.name(),
                                        primary == Some(provider),
                                    )
                                    .clicked()
                                    {
                                        primary = Some(provider);
                                        primary_changed = true;
                                    }
                                }
                            });
                            list.add_space(20.0);
                        }
                    }
                    list.label(
                        egui::RichText::new("Section order")
                            .font(theme::semibold(13.0))
                            .color(p.text),
                    );
                    list.add_space(6.0);
                    list.label(
                        egui::RichText::new("Drag a section to move it within each provider.")
                            .font(theme::regular(12.0))
                            .color(p.secondary),
                    );
                    list.add_space(8.0);
                    for (i, section) in order.iter().enumerate() {
                        let (rect, response) = list.allocate_exact_size(
                            egui::vec2(list.available_width(), ROW_HEIGHT),
                            egui::Sense::click_and_drag(),
                        );
                        if let Some(hint) = section.hint() {
                            response.clone().on_hover_text(hint);
                        }
                        if response.drag_started() {
                            egui::DragAndDrop::set_payload(list.ctx(), i);
                        }
                        let dragging_this = egui::DragAndDrop::payload::<usize>(list.ctx())
                            .is_some_and(|from| *from == i);
                        if response.hovered() || dragging_this {
                            list.painter().rect_filled(
                                rect,
                                egui::CornerRadius::same(theme::RADIUS),
                                if dragging_this {
                                    p.surface_active
                                } else {
                                    p.surface_hover
                                },
                            );
                            list.ctx().set_cursor_icon(if dragging_this {
                                egui::CursorIcon::Grabbing
                            } else {
                                egui::CursorIcon::Grab
                            });
                        }
                        paint_grip(list, rect, p.dim);
                        paint_label(list, &p, rect, *section);
                        // The drop lands on the row under the pointer; the line shows which edge
                        // of it, flipping with the direction exactly as the playlist rail does
                        // (`remove` then `insert` puts a row after the target going down, before
                        // it going up).
                        if egui::DragAndDrop::has_payload_of_type::<usize>(list.ctx())
                            && response.contains_pointer()
                        {
                            if list.ctx().input(|inp| inp.pointer.any_released()) {
                                if let Some(from) =
                                    egui::DragAndDrop::take_payload::<usize>(list.ctx())
                                {
                                    reorder = Some((*from, i));
                                }
                            } else {
                                let from = egui::DragAndDrop::payload::<usize>(list.ctx());
                                let up = from.is_some_and(|from| i < *from);
                                let y = if up { rect.top() } else { rect.bottom() };
                                list.painter().hline(
                                    egui::Rangef::new(rect.left() + 8.0, rect.right() - 8.0),
                                    y,
                                    egui::Stroke::new(2.0, p.accent),
                                );
                            }
                        }
                    }
                    // A long list: carry the scroll along while a row is dragged near an edge.
                    if egui::DragAndDrop::has_payload_of_type::<usize>(list.ctx())
                        && let Some(pos) = list.ctx().pointer_hover_pos()
                    {
                        let clip = list.clip_rect();
                        let within_x = pos.x >= clip.left() && pos.x <= clip.right();
                        if within_x && pos.y < clip.top() + EDGE {
                            list.scroll_with_delta(egui::vec2(0.0, EDGE_SPEED));
                            list.ctx().request_repaint();
                        } else if within_x && pos.y > clip.bottom() - EDGE {
                            list.scroll_with_delta(egui::vec2(0.0, -EDGE_SPEED));
                            list.ctx().request_repaint();
                        }
                    }
                });

            body.add_space(12.0);
            let (footer, _) = body.allocate_exact_size(
                egui::vec2(body.available_width(), 32.0),
                egui::Sense::hover(),
            );
            let info_rect = egui::Rect::from_min_size(
                footer.min + egui::vec2(0.0, 4.0),
                egui::Vec2::splat(24.0),
            );
            let info = body
                .interact(
                    info_rect,
                    body.id().with("home-order-info"),
                    egui::Sense::focusable_noninteractive(),
                )
                .on_hover_text(DISCLAIMER);
            info.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Label, body.is_enabled(), DISCLAIMER)
            });
            Icon::Info.image(p.secondary, 16.0).paint_at(
                body,
                egui::Rect::from_center_size(info_rect.center(), egui::Vec2::splat(16.0)),
            );
            if info.has_focus() {
                egui::Tooltip::for_widget(&info).show(|ui| {
                    ui.label(DISCLAIMER);
                });
            }
            let mut actions = body.new_child(
                egui::UiBuilder::new()
                    .id_salt("home-order-actions")
                    .max_rect(footer.with_min_x(info_rect.right() + 8.0))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            actions.spacing_mut().item_spacing.x = 8.0;
            if widgets::chip(&mut actions, &p, "Done", true).clicked() {
                close = true;
            }
            if widgets::chip(&mut actions, &p, "Reset to default", false).clicked() {
                reset = true;
            }
            if body.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });

    let ctx = ui.ctx().clone();
    if let Some(shown) = shown {
        ctx.data_mut(|data| data.insert_temp(position_id, shown.response.rect.min + drag_delta));
        if drag_delta != egui::Vec2::ZERO {
            ctx.request_repaint();
        }
    }
    if reset {
        app.update_settings(&ctx, |s| {
            s.home_source = HomeSource::Combined;
            s.home_primary_provider = None;
            s.home_order = default_order_keys();
        });
    } else {
        app.update_settings(&ctx, |s| {
            if source_changed {
                s.home_source = source;
            }
            if primary_changed {
                s.home_primary_provider = primary;
            }
            if let Some((from, to)) = reorder {
                let next = moved(&order, from, to);
                if next != order {
                    s.home_order = next.iter().map(|x| x.key().to_owned()).collect();
                }
            }
        });
    }
    if close {
        app.home_order_open = false;
    }
}

fn paint_label(ui: &egui::Ui, p: &theme::Palette, row: egui::Rect, section: HomeSection) {
    let x = row.left() + 38.0;
    let width = (row.right() - x - 12.0).max(0.0);
    ui.painter().text(
        egui::pos2(x, row.center().y),
        egui::Align2::LEFT_CENTER,
        widgets::truncate_to(ui, section.label(), theme::medium(13.0), width),
        theme::medium(13.0),
        p.text,
    );
}

fn paint_grip(ui: &egui::Ui, row: egui::Rect, colour: egui::Color32) {
    Icon::Grip.image(colour, 16.0).paint_at(
        ui,
        egui::Rect::from_center_size(
            row.left_center() + egui::vec2(16.0, 0.0),
            egui::Vec2::splat(16.0),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_custom_header_moves_the_panel_and_close_stays_separate() {
        use std::cell::Cell;

        use egui::{Event, PointerButton, Pos2, Rect, Vec2};

        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let p = theme::Palette::dark();
        let position = Cell::new(egui::pos2(60.0, 60.0));
        let header = Cell::new(Rect::NOTHING);
        let section = Cell::new(Rect::NOTHING);
        let close_button = Cell::new(Pos2::ZERO);
        let closed = Cell::new(false);
        let mut time = 0.0;
        let mut pass = |events| {
            time += 1.0 / 60.0;
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |root| {
                    let mut delta = Vec2::ZERO;
                    let mut close = false;
                    let shown = egui::Window::new("Customize Home")
                        .id(egui::Id::new("home-customizer-header-test"))
                        .title_bar(false)
                        .resizable(false)
                        .movable(false)
                        .current_pos(position.get())
                        .fixed_size(egui::vec2(320.0, 240.0))
                        .frame(egui::Frame::NONE.inner_margin(16))
                        .show(root.ctx(), |ui| {
                            (close, delta) = panel_header(ui, &p);
                            let drag = ui
                                .ctx()
                                .read_response(ui.id().with("home-order-header-drag"))
                                .unwrap();
                            header.set(drag.rect);
                            close_button.set(drag.rect.right_center() + egui::vec2(23.0, 0.0));
                            ui.add_space(16.0);
                            let (rect, response) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), ROW_HEIGHT),
                                egui::Sense::click_and_drag(),
                            );
                            section.set(rect);
                            if response.drag_started() {
                                egui::DragAndDrop::set_payload(ui.ctx(), 0_usize);
                            }
                        })
                        .unwrap();
                    position.set(shown.response.rect.min + delta);
                    closed.set(close);
                },
            )
            .drop_without_applying_deltas();
        };
        let click = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };

        pass(Vec::new());
        let before = position.get();
        let origin = header.get().center();
        let delta = egui::vec2(40.0, 24.0);
        pass(vec![Event::PointerMoved(origin)]);
        pass(vec![click(origin, true)]);
        pass(vec![Event::PointerMoved(origin + delta)]);
        pass(vec![click(origin + delta, false)]);
        assert_eq!(position.get(), before + delta);
        assert!(!closed.get());
        assert!(!egui::DragAndDrop::has_payload_of_type::<usize>(&ctx));

        let before = position.get();
        let origin = section.get().center();
        pass(vec![Event::PointerMoved(origin)]);
        pass(vec![click(origin, true)]);
        pass(vec![Event::PointerMoved(origin + delta)]);
        assert_eq!(position.get(), before);
        assert!(egui::DragAndDrop::has_payload_of_type::<usize>(&ctx));
        pass(vec![click(origin + delta, false)]);
        assert_eq!(position.get(), before);

        let origin = close_button.get();
        pass(vec![Event::PointerMoved(origin)]);
        pass(vec![click(origin, true)]);
        pass(vec![click(origin, false)]);
        assert!(closed.get());
        assert_eq!(position.get(), before);
    }

    #[test]
    fn dragging_a_row_down_puts_it_after_the_target() {
        use HomeSection::*;
        let order = vec![ListenAgain, QuickPicks, MixedForYou];
        assert_eq!(moved(&order, 0, 2), [QuickPicks, MixedForYou, ListenAgain]);
    }

    #[test]
    fn dragging_a_row_up_puts_it_before_the_target() {
        use HomeSection::*;
        let order = vec![ListenAgain, QuickPicks, MixedForYou];
        assert_eq!(moved(&order, 2, 0), [MixedForYou, ListenAgain, QuickPicks]);
    }

    #[test]
    fn dropping_a_row_on_itself_or_off_the_list_changes_nothing() {
        use HomeSection::*;
        let order = vec![ListenAgain, QuickPicks];
        assert_eq!(moved(&order, 1, 1), order);
        assert_eq!(moved(&order, 0, 9), order);
        assert_eq!(moved(&order, 9, 0), order);
    }

    /// The note names the two real reasons and the fallback group, in plain words.
    #[test]
    fn the_note_explains_batches_late_sections_and_the_fallback() {
        assert!(DISCLAIMER.contains("batches"));
        assert!(DISCLAIMER.contains("arrives late"));
        assert!(DISCLAIMER.contains("Everything else"));
    }
}
