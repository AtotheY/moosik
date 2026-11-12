//! Queue-scoped occurrence selection and typed drag/drop. Shared by the page and drawers.
use std::sync::Arc;

use crate::{icons::Icon, theme, widgets};
use egui::{Context, Id, Key, Modifiers, Rect, Ui, pos2, vec2};

#[derive(Clone, Debug)]
pub(crate) enum Action {
    Move {
        stamp: u64,
        indices: Vec<usize>,
        boundary: usize,
    },
    Remove {
        stamp: u64,
        indices: Vec<usize>,
    },
    Clear {
        stamp: u64,
    },
    Undo,
}

#[derive(Clone, Default)]
struct Selection {
    stamp: u64,
    order: Arc<Vec<usize>>,
    selected: Arc<Vec<usize>>,
    anchor: Option<usize>,
    focus: Option<Id>,
    action: Option<Action>,
}
impl Selection {
    fn choose(&mut self, index: usize, command: bool, shift: bool) {
        let selected = Arc::make_mut(&mut self.selected);
        if shift {
            let anchor = self.anchor.unwrap_or(index);
            if let (Some(a), Some(b)) = (
                self.order.iter().position(|i| *i == anchor),
                self.order.iter().position(|i| *i == index),
            ) {
                if !command {
                    selected.clear();
                }
                for i in &self.order[a.min(b)..=a.max(b)] {
                    if !selected.contains(i) {
                        selected.push(*i);
                    }
                }
            }
        } else {
            if command && selected.contains(&index) {
                selected.retain(|i| *i != index);
            } else {
                if !command {
                    selected.clear();
                }
                selected.push(index);
            }
            self.anchor = Some(index);
        }
        *selected = self
            .order
            .iter()
            .copied()
            .filter(|i| selected.contains(i))
            .collect();
    }
    fn moving(&self, boundary: usize) -> Action {
        Action::Move {
            stamp: self.stamp,
            indices: self.selected.as_ref().clone(),
            boundary,
        }
    }
}
#[derive(Clone)]
struct QueueDrag {
    scope: Scope,
    stamp: u64,
    order: Arc<Vec<usize>>,
    indices: Arc<Vec<usize>>,
    title: String,
    art: Option<egui::TextureHandle>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Scope {
    Personal,
    Session,
}
impl Scope {
    pub(crate) fn of(ui: &Ui) -> Self {
        ui.ctx()
            .data(|d| d.get_temp(ui.id().with("queue-scope")))
            .unwrap_or(Self::Personal)
    }
}

fn key(scope: Scope) -> Id {
    Id::new(("moosik-queue-selection", scope))
}
fn state(scope: Scope, ctx: &Context) -> Selection {
    ctx.data(|d| d.get_temp(key(scope)).unwrap_or_default())
}
fn save(scope: Scope, ctx: &Context, state: Selection) {
    ctx.data_mut(|d| d.insert_temp(key(scope), state));
}
fn request(scope: Scope, ctx: &Context, action: Action) {
    let mut s = state(scope, ctx);
    s.action = Some(action);
    save(scope, ctx, s);
}

pub(crate) fn clear_selection(ctx: &Context) {
    for scope in [Scope::Personal, Scope::Session] {
        ctx.data_mut(|d| d.remove::<Selection>(key(scope)));
    }
    if egui::DragAndDrop::has_payload_of_type::<QueueDrag>(ctx) {
        egui::DragAndDrop::clear_payload(ctx);
    }
}

pub(crate) fn prepare(scope: Scope, ui: &mut Ui, stamp: u64, order: &[usize]) {
    let previous = sync_order(scope, ui.ctx(), stamp, order);
    prepare_rows(scope, ui, previous);
}

/// Update the selection before any controls read it. Return the old occurrences
/// so their popups can be closed in the UI that owns the rows.
pub(crate) fn sync_order(
    scope: Scope,
    ctx: &Context,
    stamp: u64,
    order: &[usize],
) -> Option<Arc<Vec<usize>>> {
    let s = state(scope, ctx);
    if s.stamp == stamp && s.order.as_slice() == order {
        return None;
    }
    save(
        scope,
        ctx,
        Selection {
            stamp,
            order: Arc::new(order.to_vec()),
            ..Default::default()
        },
    );
    if egui::DragAndDrop::payload::<QueueDrag>(ctx).is_some_and(|d| d.scope == scope) {
        egui::DragAndDrop::clear_payload(ctx);
    }
    Some(s.order)
}

pub(crate) fn prepare_rows(scope: Scope, ui: &mut Ui, previous: Option<Arc<Vec<usize>>>) {
    ui.ctx()
        .data_mut(|d| d.insert_temp(ui.id().with("queue-scope"), scope));
    if let Some(previous) = previous {
        for index in previous.iter() {
            egui::Popup::close_id(ui.ctx(), ui.id().with(("ytm-queue-row-menu", *index)));
        }
    }
    let mut s = state(scope, ui.ctx());
    let focused = s.focus.is_some_and(|id| ui.memory(|m| m.has_focus(id)));
    let top = ui.cursor().top();
    let bottom = match scope {
        Scope::Session => {
            top + (theme::ROW_HEIGHT + ui.spacing().item_spacing.y) * s.order.len().max(1) as f32
        }
        Scope::Personal => ui.clip_rect().bottom(),
    };
    let bounds = Rect::from_min_max(
        pos2(ui.max_rect().left(), top),
        pos2(ui.max_rect().right(), bottom),
    );
    let over = ui.rect_contains_pointer(bounds.intersect(ui.clip_rect()))
        && !ui.ctx().egui_wants_keyboard_input();
    if focused || over {
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            s.selected = s.order.clone();
            s.anchor = s.order.first().copied();
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
            s.action = Some(Action::Undo);
        }
        if !s.selected.is_empty() {
            if ui.input_mut(|i| {
                i.consume_key(Modifiers::NONE, Key::Delete)
                    || i.consume_key(Modifiers::NONE, Key::Backspace)
            }) {
                s.action = Some(Action::Remove {
                    stamp: s.stamp,
                    indices: s.selected.as_ref().clone(),
                });
            }
            if ui.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowUp))
                && let Some(position) = s.order.iter().position(|i| s.selected.contains(i))
            {
                s.action = Some(s.moving(position.saturating_sub(1)));
            }
            if ui.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowDown))
                && let Some(position) = s.order.iter().rposition(|i| s.selected.contains(i))
            {
                s.action = Some(s.moving((position + 2).min(s.order.len())));
            }
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if egui::DragAndDrop::payload::<QueueDrag>(ui.ctx()).is_some_and(|d| d.scope == scope) {
                egui::DragAndDrop::clear_payload(ui.ctx());
            } else {
                s.selected = Arc::default();
                s.anchor = None;
            }
        }
    }
    save(scope, ui.ctx(), s);
}

/// Modifier clicks select without following a link or playing. A selected group is
/// frozen at drag start; pressing an already selected row never collapses it first.
pub(crate) fn interact(
    scope: Scope,
    ui: &Ui,
    response: &egui::Response,
    index: usize,
    title: &str,
    on_more: bool,
    art: Option<&egui::TextureHandle>,
) -> bool {
    let mut s = state(scope, ui.ctx());
    if !s.order.contains(&index) {
        return false;
    }
    let modifiers = ui.input(|i| i.modifiers);
    let modified = modifiers.command || modifiers.shift;
    let over = response
        .rect
        .intersect(ui.clip_rect())
        .contains(ui.input(|i| i.pointer.interact_pos().unwrap_or(pos2(-1.0, -1.0))));
    let modified_click =
        modified && over && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary));
    let mut changed = false;
    if modified_click || response.clicked() && !on_more {
        s.choose(index, modifiers.command, modifiers.shift);
        s.focus = Some(response.id);
        response.request_focus();
        changed = true;
    }
    if (response.secondary_clicked() || on_more) && !s.selected.contains(&index) {
        s.choose(index, false, false);
        changed = true;
    }
    if response.drag_started() {
        if !s.selected.contains(&index) {
            s.choose(index, false, false);
        }
        s.focus = Some(response.id);
        response.request_focus();
        changed = true;
        egui::DragAndDrop::set_payload(
            ui.ctx(),
            QueueDrag {
                scope,
                stamp: s.stamp,
                order: s.order.clone(),
                indices: s.selected.clone(),
                title: title.to_owned(),
                art: art.cloned(),
            },
        );
    }
    let dragging = egui::DragAndDrop::has_payload_of_type::<QueueDrag>(ui.ctx());
    if dragging {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        response
            .clone()
            .on_hover_text("Drag to reorder · ⌘/Ctrl-click to select · Shift-click for a range");
    }
    if changed {
        save(scope, ui.ctx(), s);
    }
    modified && over || dragging || response.drag_stopped()
}
pub(crate) fn selected(scope: Scope, ctx: &Context, index: usize) -> bool {
    state(scope, ctx).selected.contains(&index)
}

pub(crate) fn play_next(scope: Scope, ctx: &Context, index: usize) {
    let mut s = state(scope, ctx);
    if !s.selected.contains(&index) {
        s.choose(index, false, false);
    }
    request(scope, ctx, s.moving(0));
}

/// Return true for a group menu, which replaces single-song actions consistently
/// for both right-click and the row's dots button.
pub(crate) fn group_menu(
    scope: Scope,
    ui: &mut Ui,
    palette: &theme::Palette,
    index: usize,
) -> bool {
    let s = state(scope, ui.ctx());
    if !s.selected.contains(&index) || s.selected.len() < 2 {
        return false;
    }
    ui.label(
        egui::RichText::new(format!("{} songs selected", s.selected.len()))
            .font(theme::medium(12.0))
            .color(palette.secondary),
    );
    for (icon, label, action) in [
        (Icon::ListStart, "Play next", s.moving(0)),
        (
            Icon::ListX,
            "Remove from queue",
            Action::Remove {
                stamp: s.stamp,
                indices: s.selected.as_ref().clone(),
            },
        ),
    ] {
        if widgets::menu_item(ui, palette, icon, label, None).clicked() {
            request(scope, ui.ctx(), action);
            ui.close();
        }
    }
    move_menu(scope, ui, palette, &s);
    true
}
fn move_menu(scope: Scope, ui: &mut Ui, palette: &theme::Palette, s: &Selection) {
    let positions: Vec<_> = s
        .order
        .iter()
        .enumerate()
        .filter_map(|(p, i)| s.selected.contains(i).then_some(p))
        .collect();
    if positions.is_empty() {
        return;
    }
    for (label, boundary) in [
        ("Move up", positions[0].saturating_sub(1)),
        (
            "Move down",
            (positions[positions.len() - 1] + 2).min(s.order.len()),
        ),
    ] {
        if widgets::menu_item(ui, palette, Icon::ArrowUpDown, label, None).clicked() {
            request(scope, ui.ctx(), s.moving(boundary));
            ui.close();
        }
    }
}
pub(crate) fn single_move_menu(scope: Scope, ui: &mut Ui, palette: &theme::Palette) {
    move_menu(scope, ui, palette, &state(scope, ui.ctx()));
}

pub(crate) fn toolbar(
    scope: Scope,
    ui: &mut Ui,
    palette: &theme::Palette,
    title: &str,
    undo: bool,
    trailing: impl FnOnce(&mut Ui),
) {
    let mut s = state(scope, ui.ctx());
    let width = ui.available_width();
    let compact = width < 400.0;
    let selecting = !s.selected.is_empty();
    ui.allocate_ui_with_layout(
        vec2(width, 30.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            if selecting {
                selection_controls(ui, palette, compact, &mut s);
            } else {
                let heading = if compact {
                    theme::semibold(16.0)
                } else {
                    theme::bold(theme::SECTION_PT)
                };
                ui.label(egui::RichText::new(title).font(heading).color(palette.text));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                trailing(ui);
                queue_actions(ui, palette, undo, &mut s);
                if scope == Scope::Personal && !selecting && !s.order.is_empty() {
                    let count = s.order.len();
                    let label = if count == 1 { "song" } else { "songs" };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{count} {label}"))
                                .font(theme::regular(12.0))
                                .color(palette.secondary),
                        )
                        .truncate(),
                    );
                }
            });
        },
    );
    save(scope, ui.ctx(), s);
}

fn queue_actions(ui: &mut Ui, palette: &theme::Palette, undo: bool, s: &mut Selection) {
    if !s.order.is_empty() {
        let clear = widgets::icon_button(ui, palette, Icon::Trash2, 16.0, false)
            .on_hover_text("Clear upcoming songs. Keep the current song.");
        clear.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Clear queue")
        });
        if clear.clicked() {
            s.action = Some(Action::Clear { stamp: s.stamp });
        }
    }
    if undo {
        let restore = widgets::icon_button(ui, palette, Icon::History, 16.0, false)
            .on_hover_text("Undo queue edit · ⌘/Ctrl+Z");
        restore.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Undo queue edit")
        });
        if restore.clicked() {
            s.action = Some(Action::Undo);
        }
    }
}

fn selection_controls(ui: &mut Ui, palette: &theme::Palette, compact: bool, s: &mut Selection) {
    ui.label(
        egui::RichText::new(if compact {
            s.selected.len().to_string()
        } else {
            format!("{} selected", s.selected.len())
        })
        .font(theme::regular(12.0))
        .color(palette.secondary),
    );
    if widgets::icon_button(ui, palette, Icon::ListStart, 16.0, false)
        .on_hover_text("Play selected songs next")
        .clicked()
    {
        s.action = Some(s.moving(0));
    }
    if widgets::icon_button(ui, palette, Icon::ListX, 16.0, false)
        .on_hover_text("Remove selected songs · Delete")
        .clicked()
    {
        s.action = Some(Action::Remove {
            stamp: s.stamp,
            indices: s.selected.as_ref().clone(),
        });
    }
    if !compact
        && widgets::icon_button(ui, palette, Icon::X, 16.0, false)
            .on_hover_text("Clear selection")
            .clicked()
    {
        s.selected = Arc::default();
    }
}

/// Return the insertion point under the pointer, including the open space below the last row.
pub(crate) fn drop_boundary(
    ui: &mut Ui,
    palette: &theme::Palette,
    top: f32,
    count: usize,
) -> Option<usize> {
    boundary_in(ui, palette, top, count, ui.clip_rect().bottom())
}

fn boundary_in(
    ui: &mut Ui,
    palette: &theme::Palette,
    top: f32,
    count: usize,
    bottom: f32,
) -> Option<usize> {
    let pointer = ui.input(|i| i.pointer.interact_pos())?;
    let clip = ui.clip_rect();
    let bounds = Rect::from_min_max(
        pos2(ui.min_rect().left(), top),
        pos2(ui.max_rect().right(), bottom),
    )
    .intersect(clip);
    let hovered = ui.rect_contains_pointer(bounds);
    super::song_drag::drop_outline(ui, palette, bounds, hovered);
    if !hovered {
        return None;
    }
    let pitch = theme::ROW_HEIGHT + ui.spacing().item_spacing.y;
    let boundary = (((pointer.y - top) / pitch).round() as usize).min(count);
    let y = (top + boundary as f32 * pitch - 2.0).clamp(bounds.top(), bounds.bottom());
    ui.painter()
        .hline(bounds.x_range(), y, egui::Stroke::new(2.0, palette.accent));
    let edge = 44.0;
    let speed = if pointer.y < clip.top() + edge {
        (clip.top() + edge - pointer.y) / edge
    } else if pointer.y > clip.bottom() - edge {
        -(pointer.y - clip.bottom() + edge) / edge
    } else {
        0.0
    };
    if speed != 0.0 {
        ui.scroll_with_delta(vec2(0.0, speed * 9.0));
        ui.ctx().request_repaint();
    }
    Some(boundary)
}

pub(crate) fn drop_target(
    scope: Scope,
    ui: &mut Ui,
    palette: &theme::Palette,
    top: f32,
    count: usize,
) {
    let Some(drag) = egui::DragAndDrop::payload::<QueueDrag>(ui.ctx()) else {
        return;
    };
    if drag.scope != scope {
        return;
    }
    let s = state(scope, ui.ctx());
    if drag.stamp != s.stamp || drag.order != s.order {
        egui::DragAndDrop::clear_payload(ui.ctx());
        return;
    }
    let bottom = if scope == Scope::Session {
        ui.min_rect().bottom()
    } else {
        ui.clip_rect().bottom()
    };
    if let Some(boundary) = boundary_in(ui, palette, top, count, bottom)
        && ui.input(|i| i.pointer.primary_released())
    {
        request(
            scope,
            ui.ctx(),
            Action::Move {
                stamp: drag.stamp,
                indices: drag.indices.as_ref().clone(),
                boundary,
            },
        );
        egui::DragAndDrop::clear_payload(ui.ctx());
        return;
    }
    let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) else {
        return;
    };
    if ui.input(|i| i.pointer.any_released()) {
        // Another visible queue view can still receive this release before end-of-frame cleanup.
        return;
    }
    // A compact floating label, outside normal layout and without hit testing.
    let label = if drag.indices.len() > 1 {
        format!("{} songs", drag.indices.len())
    } else {
        drag.title.clone()
    };
    let layer = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        Id::new("queue-drag-preview"),
    ));
    let text = widgets::truncate_to(ui, &label, theme::medium(12.0), 220.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.into_owned(), theme::medium(12.0), palette.text);
    let rect = Rect::from_min_size(
        pointer + vec2(14.0, 18.0),
        vec2(galley.size().x + 64.0, 46.0),
    );
    layer.rect_filled(rect, theme::RADIUS, palette.surface_active);
    let cover = Rect::from_min_size(rect.min + vec2(7.0, 7.0), vec2(32.0, 32.0));
    if let Some(art) = &drag.art {
        layer.image(
            art.id(),
            cover,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        layer.rect_filled(cover, theme::RADIUS_SMALL, palette.surface_hover);
    }
    layer.galley(
        pos2(
            cover.right() + 10.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        palette.text,
    );
}
pub(crate) fn take_action(scope: Scope, ctx: &Context) -> Option<Action> {
    let mut s = state(scope, ctx);
    let action = s.action.take();
    save(scope, ctx, s);
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(ctx: &Context, events: Vec<egui::Event>, stamp: u64) -> Vec<Rect> {
        let mut rects = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(500.0, 400.0))),
                events,
                ..Default::default()
            },
            |ui| {
                prepare(Scope::Personal, ui, stamp, &[10, 20, 30]);
                let top = ui.cursor().top();
                for index in [10, 20, 30] {
                    let (rect, response) = ui.allocate_exact_size(
                        vec2(ui.available_width(), theme::ROW_HEIGHT),
                        egui::Sense::click_and_drag(),
                    );
                    interact(Scope::Personal, ui, &response, index, "Song", false, None);
                    rects.push(rect);
                }
                drop_target(Scope::Personal, ui, &theme::Palette::dark(), top, 3);
            },
        );
        output.textures_delta.clear();
        rects
    }
    fn pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }
    #[test]
    fn session_toolbar_clear_uses_the_order_received_on_click_release() {
        let ctx = Context::default();
        theme::install_fonts(&ctx);
        crate::icons::register(&ctx);
        let draw = |events, stamp, order: &[usize]| {
            let mut clear = egui::Pos2::ZERO;
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(500.0, 400.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let previous = sync_order(Scope::Session, &ctx, stamp, order);
                    toolbar(
                        Scope::Session,
                        ui,
                        &theme::Palette::dark(),
                        "Session queue",
                        false,
                        |ui| {
                            clear = ui.available_rect_before_wrap().right_top() + vec2(-15.0, 15.0);
                        },
                    );
                    egui::Frame::new().show(ui, |ui| prepare_rows(Scope::Session, ui, previous));
                },
            )
            .drop_without_applying_deltas();
            clear
        };
        let clear = draw(vec![], 1, &[10]);
        draw(
            vec![egui::Event::PointerMoved(clear), pointer(clear, true)],
            1,
            &[10],
        );
        draw(vec![pointer(clear, false)], 2, &[10, 20]);
        assert!(matches!(
            take_action(Scope::Session, &ctx),
            Some(Action::Clear { stamp: 2 })
        ));
    }

    #[test]
    fn dragging_selected_songs_keeps_the_group_and_can_append() {
        let ctx = Context::default();
        theme::install_fonts(&ctx);
        let rects = frame(&ctx, vec![], 7);
        let mut s = state(Scope::Personal, &ctx);
        s.selected = Arc::new(vec![10, 30]);
        save(Scope::Personal, &ctx, s);
        let start = rects[0].center();
        // Drop in the open space well below the final row.
        let end = rects[2].center_bottom() + vec2(0.0, 110.0);
        frame(&ctx, vec![egui::Event::PointerMoved(start)], 7);
        frame(&ctx, vec![pointer(start, true)], 7);
        frame(
            &ctx,
            vec![egui::Event::PointerMoved(start + vec2(0.0, 15.0))],
            7,
        );
        assert_eq!(
            egui::DragAndDrop::payload::<QueueDrag>(&ctx)
                .unwrap()
                .indices
                .as_slice(),
            &[10, 30]
        );
        frame(&ctx, vec![egui::Event::PointerMoved(end)], 7);
        frame(&ctx, vec![pointer(end, false)], 7);
        match take_action(Scope::Personal, &ctx).unwrap() {
            Action::Move {
                indices, boundary, ..
            } => {
                assert_eq!(indices, vec![10, 30]);
                assert_eq!(boundary, 3);
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }
    #[test]
    fn real_song_rows_drag_inside_scroll_area_without_playing() {
        let ctx = Context::default();
        theme::install_fonts(&ctx);
        egui_extras::install_image_loaders(&ctx);
        crate::icons::register(&ctx);
        let palette = theme::Palette::dark();
        let track = crate::model::Track {
            video_id: "test".into(),
            title: "Song".into(),
            artists: vec![],
            album: None,
            album_id: None,
            duration: Some(180),
            plays: None,
            explicit: false,
            like: Default::default(),
            in_library: false,
            library_add: None,
            library_remove: None,
            playlist_added_ms: None,
            set_video_id: None,
            thumbnail: None,
            wide: false,
            rank: None,
            trend: crate::model::ChartTrend::Unknown,
            local_path: None,
        };
        let mut time = 0.0;
        let mut frame = |events| {
            time += 1.0 / 60.0;
            let mut rects = Vec::new();
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        prepare(Scope::Personal, ui, 7, &[10, 20, 30]);
                        toolbar(Scope::Personal, ui, &palette, "Next up", false, |_| {});
                        let top = ui.cursor().top();
                        for (i, index) in [10, 20, 30].into_iter().enumerate() {
                            let hit = widgets::track_row(
                                ui,
                                &palette,
                                widgets::TrackRow {
                                    number: None,
                                    rank: None,
                                    trend: track.trend,
                                    title: &track.title,
                                    source: Some(track.source_name()),
                                    album_id: None,
                                    subtitle: widgets::plain_subtitle("Artist"),
                                    duration: track.duration,
                                    plays: None,
                                    explicit: false,
                                    active: false,
                                    playing: false,
                                    badge: None,
                                    art: None,
                                    shape: widgets::ArtShape::Square,
                                    hide_art: false,
                                    id_salt: i,
                                    menu: Some(widgets::RowMenu {
                                        track: &track,
                                        playlist_id: None,
                                        playlists: &[],
                                        signed_in: false,
                                        queue_index: Some(index),
                                    }),
                                    like: track.like,
                                    show_rating: false,
                                    quiet_like: false,
                                    add_to: None,
                                },
                            );
                            assert!(!hit.activate, "drag must not start playback");
                            rects.push(hit.response.rect);
                        }
                        drop_target(Scope::Personal, ui, &palette, top, 3);
                    });
                },
            )
            .drop_without_applying_deltas();
            rects
        };
        let rects = frame(vec![]);
        let start = rects[0].center();
        let end = rects[2].center() + vec2(0.0, 20.0);
        frame(vec![egui::Event::PointerMoved(start)]);
        frame(vec![pointer(start, true)]);
        frame(vec![egui::Event::PointerMoved(start + vec2(0.0, 15.0))]);
        assert!(egui::DragAndDrop::has_payload_of_type::<QueueDrag>(&ctx));
        let dragging = frame(vec![egui::Event::PointerMoved(end)]);
        assert_eq!(dragging[2].top(), rects[2].top());
        frame(vec![pointer(end, false)]);
        assert!(matches!(
            take_action(Scope::Personal, &ctx),
            Some(Action::Move { boundary: 3, .. })
        ));
    }
    #[test]
    fn escape_stale_queue_and_outside_drops_cannot_apply_a_move() {
        let ctx = Context::default();
        theme::install_fonts(&ctx);
        frame(&ctx, vec![], 7);
        let mut s = state(Scope::Personal, &ctx);
        s.selected = Arc::new(vec![10, 30]);
        save(Scope::Personal, &ctx, s);
        let drag = QueueDrag {
            scope: Scope::Personal,
            stamp: 7,
            order: Arc::new(vec![10, 20, 30]),
            indices: Arc::new(vec![10, 30]),
            title: "Two songs".into(),
            art: None,
        };
        egui::DragAndDrop::set_payload(&ctx, drag.clone());
        frame(
            &ctx,
            vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            7,
        );
        assert!(!egui::DragAndDrop::has_payload_of_type::<QueueDrag>(&ctx));
        assert_eq!(state(Scope::Personal, &ctx).selected.as_slice(), &[10, 30]);
        assert!(take_action(Scope::Personal, &ctx).is_none());
        egui::DragAndDrop::set_payload(&ctx, drag.clone());
        frame(&ctx, vec![], 8);
        assert!(!egui::DragAndDrop::has_payload_of_type::<QueueDrag>(&ctx));
        egui::DragAndDrop::set_payload(&ctx, drag);
        frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(pos2(700.0, 600.0)),
                pointer(pos2(700.0, 600.0), false),
            ],
            7,
        );
        assert!(take_action(Scope::Personal, &ctx).is_none());
        egui::DragAndDrop::set_payload(&ctx, 42usize);
        frame(&ctx, vec![], 9);
        assert_eq!(*egui::DragAndDrop::payload::<usize>(&ctx).unwrap(), 42);
    }

    #[test]
    fn ranges_and_disjoint_occurrences_keep_display_order() {
        let mut s = Selection {
            order: Arc::new(vec![5, 2, 8, 1, 9]),
            ..Default::default()
        };
        s.choose(2, false, false);
        s.choose(1, true, false);
        assert_eq!(s.selected.as_slice(), &[2, 1]);
        s.choose(5, false, true);
        assert_eq!(s.selected.as_slice(), &[5, 2, 8, 1]);
        s.choose(8, true, false);
        assert_eq!(s.selected.as_slice(), &[5, 2, 1]);
        s.choose(9, true, true);
        assert_eq!(s.selected.as_slice(), &[5, 2, 8, 1, 9]);
    }
}
