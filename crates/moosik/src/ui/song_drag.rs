//! Song transfer shared by ordinary rows, the queue and playlist drop targets.
use crate::{
    model::{PlaylistSource, Track},
    theme,
};
use egui::{Context, Id, Rect, Ui};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Payload {
    pub tracks: Vec<Track>,
    pub source: Id,
    pub playlist: Option<String>,
    pub queue: bool,
}
impl Payload {
    pub(crate) fn session_eligible(&self) -> bool {
        self.tracks.iter().all(Track::can_share_in_session)
    }

    pub(crate) fn accepts_playlist(
        &self,
        id: &str,
        source: PlaylistSource,
        signed_in: bool,
    ) -> bool {
        if self.playlist.as_deref() == Some(id) {
            return false;
        }
        if id.starts_with("private:") {
            return true;
        }
        match source {
            PlaylistSource::Local => self.tracks.iter().all(|t| t.local_path.is_some()),
            PlaylistSource::YouTube => {
                signed_in
                    && self
                        .tracks
                        .iter()
                        .all(|t| t.local_path.is_none() && !t.is_spotify())
            }
            PlaylistSource::Spotify => false,
        }
    }
}

#[derive(Clone, Default)]
struct Selection {
    list: Option<Id>,
    indices: Vec<usize>,
    anchor: usize,
}
fn key() -> Id {
    Id::new("moosik-song-transfer")
}
fn selection_key() -> Id {
    key().with("selection")
}
pub(crate) fn payload(ctx: &Context) -> Option<Arc<Payload>> {
    ctx.data(|d| d.get_temp(key()))
}
pub(crate) fn start(ctx: &Context, payload: Payload) {
    ctx.data_mut(|d| d.insert_temp(key(), Arc::new(payload)));
}
pub(crate) fn take(ctx: &Context) -> Option<Payload> {
    let value = payload(ctx);
    ctx.data_mut(|d| d.remove::<Arc<Payload>>(key()));
    if value.is_some() {
        egui::DragAndDrop::clear_payload(ctx);
    }
    value.map(Arc::unwrap_or_clone)
}
pub(crate) fn clear_selection(ctx: &Context) {
    ctx.data_mut(|d| d.remove::<Selection>(selection_key()));
}
/// Session selections use row indices. Discard them before drawing a different ordering.
pub(crate) fn sync_list_order(ctx: &Context, list: Id, order: Id) {
    ctx.data_mut(|data| {
        let key = list.with("song-selection-order");
        if data.get_temp::<Id>(key) == Some(order) {
            return;
        }
        data.insert_temp(key, order);
        if data
            .get_temp::<Selection>(selection_key())
            .is_some_and(|s| s.list == Some(list))
        {
            data.remove::<Selection>(selection_key());
        }
    });
}
pub(crate) fn selected(ctx: &Context, list: Id, index: usize) -> bool {
    ctx.data(|d| d.get_temp::<Selection>(selection_key()))
        .is_some_and(|s| s.list == Some(list) && s.indices.contains(&index))
}
/// Modified clicks select without playing; selection is scoped to this exact list.
pub(crate) fn interact(
    ui: &Ui,
    response: &egui::Response,
    index: usize,
    track: &Track,
    playlist: Option<&str>,
    on_more: bool,
) -> bool {
    let list = ui.id();
    let mut s = ui
        .ctx()
        .data(|d| d.get_temp::<Selection>(selection_key()))
        .unwrap_or_default();
    if s.list != Some(list) {
        s = Selection {
            list: Some(list),
            ..Default::default()
        };
    }
    let modifiers = ui.input(|i| i.modifiers);
    let modified = modifiers.command || modifiers.shift;
    let over = ui.rect_contains_pointer(response.rect);
    let clicked = response.clicked()
        || (over
            && modified
            && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)));
    if clicked && !on_more {
        if modifiers.shift {
            if !modifiers.command {
                s.indices.clear();
            }
            for i in s.anchor.min(index)..=s.anchor.max(index) {
                if !s.indices.contains(&i) {
                    s.indices.push(i);
                }
            }
        } else if modifiers.command {
            if s.indices.contains(&index) {
                s.indices.retain(|i| *i != index);
            } else {
                s.indices.push(index);
            }
            s.anchor = index;
        } else {
            s.indices = vec![index];
            s.anchor = index;
        }
        s.indices.sort_unstable();
        ui.ctx()
            .data_mut(|d| d.insert_temp(selection_key(), s.clone()));
    }
    if response.drag_started() {
        if !s.indices.contains(&index) {
            s.indices = vec![index];
            s.anchor = index;
            ui.ctx().data_mut(|d| d.insert_temp(selection_key(), s));
        }
        start(
            ui.ctx(),
            Payload {
                tracks: vec![track.clone()],
                source: list,
                playlist: playlist.map(str::to_owned),
                queue: false,
            },
        );
    }
    (modified && over) || payload(ui.ctx()).is_some() || response.drag_stopped()
}
pub(crate) fn indices(ctx: &Context, list: Id) -> Vec<usize> {
    ctx.data(|d| d.get_temp::<Selection>(selection_key()))
        .filter(|s| s.list == Some(list))
        .map(|s| s.indices)
        .unwrap_or_default()
}
/// Mark every eligible destination throughout a drag; emphasize the one under the pointer.
pub(crate) fn drop_outline(ui: &Ui, palette: &theme::Palette, rect: Rect, hovered: bool) {
    if !ui.is_rect_visible(rect) || rect.width() < 5.0 || rect.height() < 5.0 {
        return;
    }
    let mut path = Vec::new();
    egui::epaint::tessellator::path::rounded_rectangle(
        &mut path,
        rect.shrink(2.0),
        egui::CornerRadius::same(theme::RADIUS).into(),
    );
    path.push(path[0]);
    let (color, radius) = if hovered {
        (palette.accent, 1.6)
    } else {
        (palette.accent.gamma_multiply(0.65), 1.1)
    };
    ui.painter()
        .extend(egui::Shape::dotted_line(&path, color, 6.0, radius));
}

/// Targets use geometry so children (artwork and links) cannot steal the drop.
pub(crate) fn target(
    ui: &Ui,
    palette: &theme::Palette,
    rect: Rect,
    enabled: bool,
) -> Option<Vec<Track>> {
    if !enabled || payload(ui.ctx()).is_none() {
        return None;
    }
    let hovered = ui.rect_contains_pointer(rect);
    drop_outline(ui, palette, rect, hovered);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
        if ui.input(|i| i.pointer.primary_released() && !i.key_pressed(egui::Key::Escape)) {
            return take(ui.ctx()).map(|drag| drag.tracks);
        }
    }
    None
}
pub(crate) fn finish(ctx: &Context, palette: &theme::Palette) {
    let Some(drag) = payload(ctx) else {
        return;
    };
    if ctx.input(|i| {
        i.key_pressed(egui::Key::Escape) || i.pointer.any_released() || !i.pointer.any_down()
    }) {
        take(ctx);
        return;
    }
    if !drag.queue
        && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
    {
        if ctx.output(|o| o.cursor_icon != egui::CursorIcon::Copy) {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        egui::Area::new(key().with("preview"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + egui::vec2(16.0, 16.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(palette.overlay)
                    .corner_radius(theme::RADIUS)
                    .inner_margin(12.0)
                    .stroke(egui::Stroke::new(1.0, palette.accent))
                    .show(ui, |ui| {
                        let text = if drag.tracks.len() == 1 {
                            drag.tracks[0].title.clone()
                        } else {
                            format!("{} songs", drag.tracks.len())
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
                                    .font(theme::semibold(13.0))
                                    .color(palette.text),
                            )
                            .truncate(),
                        );
                    });
            });
    }
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets;
    use egui::{pos2, vec2};
    #[test]
    fn host_advancing_clears_only_the_changed_session_selection() {
        let ctx = Context::default();
        let session = Id::new("session");
        let before = Id::new([1, 2, 3, 4]);
        let after = Id::new([2, 3, 4]);
        sync_list_order(&ctx, session, before);
        ctx.data_mut(|data| {
            data.insert_temp(
                selection_key(),
                Selection {
                    list: Some(session),
                    indices: vec![1, 2],
                    anchor: 1,
                },
            )
        });
        sync_list_order(&ctx, session, before);
        sync_list_order(&ctx, Id::new("other-view"), after);
        assert_eq!(indices(&ctx, session), [1, 2]);
        sync_list_order(&ctx, session, after);
        assert!(indices(&ctx, session).is_empty());
    }

    #[test]
    fn shared_row_modified_clicks_and_drag_do_not_play() {
        let ctx = Context::default();
        theme::install_fonts(&ctx);
        egui_extras::install_image_loaders(&ctx);
        crate::icons::register(&ctx);
        let palette = theme::Palette::dark();
        let track = crate::listening::Metadata {
            id: "song".into(),
            title: "Song".into(),
            ..Default::default()
        }
        .to_track();
        let mut time = 0.0;
        let mut frame = |mut events: Vec<egui::Event>, modifiers| {
            events.insert(0, egui::Event::ModifiersChanged(modifiers));
            time += 1.0 / 60.0;
            let mut rects = vec![];
            let mut dropped = None;
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    dropped = target(
                        ui,
                        &palette,
                        Rect::from_min_size(pos2(20.0, 300.0), vec2(400.0, 60.0)),
                        true,
                    );
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for i in 0..3 {
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
                                    duration: None,
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
                                        queue_index: None,
                                    }),
                                    like: track.like,
                                    show_rating: false,
                                    quiet_like: false,
                                    add_to: None,
                                },
                            );
                            assert!(!hit.activate, "a selection or drag must not start playback");
                            rects.push(hit.response.rect);
                        }
                    });
                    finish(ui.ctx(), &palette);
                },
            )
            .drop_without_applying_deltas();
            (rects, dropped)
        };
        let modifiers = egui::Modifiers::COMMAND;
        let (rects, _) = frame(vec![], modifiers);
        let start = rects[0].center();
        let pointer = |pos, pressed, modifiers| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        };
        frame(vec![egui::Event::PointerMoved(start)], modifiers);
        frame(vec![pointer(start, true, modifiers)], modifiers);
        frame(vec![pointer(start, false, modifiers)], modifiers);
        assert_eq!(
            ctx.data(|d| d.get_temp::<Selection>(selection_key()))
                .unwrap()
                .indices,
            vec![0]
        );
        let last = rects[2].center();
        frame(
            vec![egui::Event::PointerMoved(last)],
            egui::Modifiers::SHIFT,
        );
        frame(
            vec![pointer(last, true, egui::Modifiers::SHIFT)],
            egui::Modifiers::SHIFT,
        );
        frame(
            vec![pointer(last, false, egui::Modifiers::SHIFT)],
            egui::Modifiers::SHIFT,
        );
        assert_eq!(
            ctx.data(|d| d.get_temp::<Selection>(selection_key()))
                .unwrap()
                .indices,
            vec![0, 1, 2]
        );
        frame(
            vec![egui::Event::PointerMoved(start)],
            egui::Modifiers::NONE,
        );
        frame(
            vec![pointer(start, true, egui::Modifiers::NONE)],
            egui::Modifiers::NONE,
        );
        frame(
            vec![egui::Event::PointerMoved(start + vec2(0.0, 16.0))],
            egui::Modifiers::NONE,
        );
        assert!(payload(&ctx).is_some());
        let end = pos2(200.0, 330.0);
        frame(vec![egui::Event::PointerMoved(end)], egui::Modifiers::NONE);
        let (_, dropped) = frame(
            vec![pointer(end, false, egui::Modifiers::NONE)],
            egui::Modifiers::NONE,
        );
        assert_eq!(dropped.unwrap()[0].video_id, "song");
        assert!(payload(&ctx).is_none());
    }
}
