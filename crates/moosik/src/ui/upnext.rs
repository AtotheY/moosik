//! The queue: what is playing, and what follows.
//!
//! Reuses `spec_rows` so virtualisation, links and art budgets apply unchanged.
//!
//! No rating thumbs here (`show_rating: false`). This is a list of what happens next, and a
//! column of controls repeated down it is noise; rating a queued song is one right-click away.

use crate::app::{App, SessionMember};
use crate::theme;
use crate::ui::{ListClick, RowSpec, centered, is_active, spec_rows, track_subtitle_no_album};
use crate::widgets::{self, ArtShape};
use crate::{backend::Backend, images::Images};

pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    // The card's own rect, before the list consumes the ui: the corner repaint at the end
    // needs its rounded edges. Taken before the empty queue returns, so that card has its edge
    // too. Not in the fullscreen drawer, which is no card (see the notches below).
    let card = widgets::card_rect(ui.available_rect_before_wrap(), theme::CARD_INSET);
    external_drop(app, ui, card);
    let _edge = (!app.fs.open).then(|| widgets::CardEdge::new(ui, card, &app.palette));
    if app.q.is_empty() && app.room_snapshot().is_none() && !app.following() {
        edit_controls(app, ui, &[], true);
        centered(app, ui, "Empty queue");
        return;
    }
    widgets::clip_card_content(ui, card, theme::CARD_INSET_X, &app.palette);

    let mut area = egui::ScrollArea::vertical()
        .id_salt("upnext")
        .auto_shrink([false, false]);
    if app.queue_scroll_reset {
        area = area.vertical_scroll_offset(0.0);
        app.queue_scroll_reset = false;
    }
    let from = widgets::painted_so_far(ui);
    let out = area.show(ui, |ui| {
        // The list runs to the card's top edge now; this is the header's own breathing room,
        // and it scrolls away with it — the same distance the content has from the sides.
        ui.add_space(8.0);
        now_playing(app, ui, false);
        session_queue(app, ui, false);
        if app.following() && app.room_snapshot().is_some() {
            return;
        }
        let label = if app.room_snapshot().is_some() {
            "Personal queue"
        } else {
            "Next up"
        };
        section(app, ui, 8.0, label);
        let upcoming = app
            .q
            .upcoming(app.repeat, app.radio.as_ref().map(|r| &r.chosen));
        next_up(app, ui, &upcoming);
    });
    // Where the list is scrolled to, for the fullscreen view's edge fades: they only belong where
    // there is more to scroll to.
    app.fs.queue_scroll = (
        out.state.offset.y,
        out.content_size.y,
        out.inner_rect.height(),
    );
    // The card fades its list at an end with more of it past that end. Not in the fullscreen
    // drawer, which fades the whole panel itself: twice would double it.
    if !app.fs.open {
        widgets::fade_scroll_edges(ui, from, &out, widgets::scroll_fade(&app.settings));
    }
    // The card's rounded corners, over everything the queue drew: the list runs to the card's
    // top and bottom edges now.
    // Not in the fullscreen drawer: it is a card on a tinted background, held in from its own
    // rounded ends, and the gutter colour would show as a notch at each corner.
    if !app.fs.open {
        widgets::paint_corner_notches(ui, card, theme::CARD_INSET_X, &app.palette);
    }
}

/// A small caps-ish section label, as the reference uses above each block. `gap` is the space
/// above it: sections that follow a row carry 8, the first block under the panel's top carries
/// none — the queue is dismissed from the player bar's list button, so there is no header row
/// to clear.
fn section(app: &App, ui: &mut egui::Ui, gap: f32, label: &str) {
    ui.add_space(gap);
    ui.label(
        egui::RichText::new(label)
            .font(theme::semibold(12.0))
            .color(app.palette.secondary),
    );
    ui.add_space(4.0);
}

const AVATAR: f32 = 24.0;
const AVATAR_GAP: f32 = 4.0;

fn avatar(
    ui: &mut egui::Ui,
    p: &theme::Palette,
    images: &mut Images,
    backend: &Backend,
    member: &SessionMember,
    label: &str,
) {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(AVATAR), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        if let Some(tex) =
            crate::ui::art_texture(images, backend, ui.ctx(), member.photo.as_ref(), AVATAR)
        {
            widgets::paint_art(ui, p, rect, Some(tex), ArtShape::Circle);
        } else {
            ui.painter()
                .circle_filled(rect.center(), AVATAR / 2.0, p.surface_active);
            let initial = member
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                initial,
                theme::semibold(12.0),
                p.text,
            );
        }
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Image, true, label));
    response.on_hover_text(label);
}

fn playing_header(app: &mut App, ui: &mut egui::Ui, title: &str, page: bool) {
    if page {
        ui.add_space(20.0);
    }
    let p = app.palette;
    ui.horizontal(|ui| {
        if !page {
            ui.add_space(2.0);
        }
        let text = egui::RichText::new(title);
        ui.label(if page {
            text.font(theme::bold(theme::SECTION_PT)).color(p.text)
        } else {
            text.font(theme::semibold(12.0)).color(p.secondary)
        });
        let Some(room) = app.together.snapshot() else {
            return;
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = AVATAR_GAP;
            let slots =
                ((ui.available_width() + AVATAR_GAP) / (AVATAR + AVATAR_GAP)).floor() as usize;
            let members = &room.members;
            let visible = if members.len() <= slots {
                members.len()
            } else {
                slots.saturating_sub(1)
            };
            let label = |member: &SessionMember| {
                let role = match (member.id == 0, member.id == room.you) {
                    (true, true) => " (you, host)",
                    (true, false) => " (host)",
                    (false, true) => " (you)",
                    _ => "",
                };
                format!("{}{role}", member.name)
            };
            if visible < members.len() {
                let names = members[visible..]
                    .iter()
                    .map(label)
                    .collect::<Vec<_>>()
                    .join("\n");
                ui.label(
                    egui::RichText::new(format!("+{}", members.len() - visible))
                        .font(theme::semibold(12.0))
                        .color(p.secondary),
                )
                .on_hover_text(names);
            }
            for member in members[..visible].iter().rev() {
                avatar(
                    ui,
                    &p,
                    &mut app.images,
                    &app.backend,
                    member,
                    &label(member),
                );
            }
        });
    });
    ui.add_space(if page { 8.0 } else { 4.0 });
}

/// The track the transport is on, above the list of what follows it.
fn now_playing(app: &mut App, ui: &mut egui::Ui, page: bool) {
    let Some(track) = app.q.current_track().cloned() else {
        if app.room_snapshot().is_some() {
            playing_header(app, ui, "Listening session", page);
        }
        return;
    };
    playing_header(app, ui, "Now playing", page);
    let credit = app
        .room_snapshot()
        .and_then(|room| room.playing_credit(&track.video_id))
        .map(|(member, label)| (member.clone(), label));
    // Namespaced under its own id: this row and "Next up"'s first row both start their own
    // `spec_rows` call at index 0, so without this their title/artist links resolve to the
    // exact same widget id (`ui.id()` is the same parent for both — they're sequential calls on
    // the same `Ui`, not separately salted children) and neither one's link reliably works.
    let signed_in = app.signed_in();
    let row = |app: &mut App, ui: &mut egui::Ui| {
        ui.push_id("now-playing", |ui| {
            let App {
                palette,
                images,
                backend,
                stats,
                liked,
                now,
                library,
                ..
            } = app;
            let subtitle = track_subtitle_no_album(&track);
            spec_rows(ui, palette, images, backend, stats, liked, 1, |_| RowSpec {
                number: None,
                rank: track.rank.as_deref(),
                trend: track.trend,
                title: &track.title,
                album_id: track.album_id.as_deref(),
                subtitle,
                duration: track.duration,
                plays: None,
                explicit: track.explicit,
                active: is_active(now, &track.video_id),
                playing: is_active(now, &track.video_id) && !now.paused,
                badge: None,
                // Square: the cover, not a cropped 16:9 still.
                shape: ArtShape::Square,
                hide_art: false,
                thumbnail: track.thumbnail.as_ref(),
                // The same menu as the rows below, without Remove from Queue: the playing track
                // is not removed from under itself.
                menu: Some(widgets::RowMenu {
                    track: &track,
                    playlist_id: None,
                    playlists: &library.playlists.items,
                    signed_in,
                    queue_index: None,
                }),
                like: track.like,
                show_rating: false,
                quiet_like: false,
                add_to: None,
            })
        })
        .inner
    };
    let clicked = if let Some((member, label)) = credit {
        ui.horizontal(|ui| {
            let width = ui.available_width() - AVATAR - ui.spacing().item_spacing.x;
            let clicked = ui
                .allocate_ui_with_layout(
                    egui::vec2(width.max(0.0), theme::ROW_HEIGHT),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| row(app, ui),
                )
                .inner;
            avatar(
                ui,
                &app.palette,
                &mut app.images,
                &app.backend,
                &member,
                &label,
            );
            clicked
        })
        .inner
    } else {
        row(app, ui)
    };
    if let Some(action) = clicked.action {
        app.row_action(action);
    }
    match clicked.click {
        Some(ListClick::Link(route)) => app.navigate(route),
        // Elsewhere on the row: it's already what's playing, so a click means pause/resume,
        // same as clicking it anywhere else it appears.
        Some(ListClick::Row(_)) => app.toggle_playback(),
        None => {}
    }
}

/// The full queue uses the same live ordering and actions as the compact panel.
pub fn page(app: &mut App, ui: &mut egui::Ui) {
    external_drop(app, ui, ui.available_rect_before_wrap());
    crate::ui::page_scroll(app, "queue-page").show(ui, |ui| {
        crate::ui::page_title(ui, &app.palette, "Queue");
        if app.q.current_track().is_none() && app.q.tracks().is_empty() {
            if app.following() || app.room_snapshot().is_some() {
                now_playing(app, ui, true);
                session_queue(app, ui, true);
                return;
            }
            edit_controls(app, ui, &[], false);
            centered(app, ui, "Your queue is empty. Play a song to get started.");
            return;
        }
        if widgets::pill_button(ui, &app.palette, "Save as private playlist").clicked() {
            let tracks = app
                .q
                .current_track()
                .cloned()
                .into_iter()
                .chain(
                    app.room_snapshot()
                        .into_iter()
                        .flat_map(|room| room.queue.iter().map(|e| e.song.track())),
                )
                .chain(
                    app.q
                        .upcoming(app.repeat, app.radio.as_ref().map(|r| &r.chosen))
                        .into_iter()
                        .map(|i| app.q.tracks()[i].clone()),
                )
                .collect();
            app.new_private_playlist(tracks, None);
        }
        ui.add_space(12.0);
        now_playing(app, ui, true);
        session_queue(app, ui, true);
        if app.following() && app.room_snapshot().is_some() {
            return;
        }
        let upcoming = app
            .q
            .upcoming(app.repeat, app.radio.as_ref().map(|r| &r.chosen));
        let label = if app.room_snapshot().is_some() {
            "Personal queue"
        } else {
            "Next up"
        };
        crate::ui::section(ui, &app.palette, label);
        if upcoming.is_empty() {
            edit_controls(app, ui, &upcoming, false);
            ui.label(
                egui::RichText::new("Nothing up next")
                    .font(theme::regular(13.0))
                    .color(app.palette.secondary),
            );
        } else {
            next_up(app, ui, &upcoming);
        }
    });
}

fn session_queue(app: &mut App, ui: &mut egui::Ui, page: bool) {
    if app.room_snapshot().is_none() && !app.following() {
        return;
    }
    let p = app.palette;
    let title = if app.room_snapshot().is_some() {
        "Session queue"
    } else {
        "Listening along"
    };
    ui.add_space(if page { 20.0 } else { 8.0 });
    ui.horizontal(|ui| {
        ui.label(if page {
            egui::RichText::new(title)
                .font(theme::bold(theme::SECTION_PT))
                .color(p.text)
        } else {
            egui::RichText::new(title)
                .font(theme::semibold(12.0))
                .color(p.secondary)
        });
        if app.following() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::outline_button(ui, &p, "Leave")
                    .on_hover_text("Leave listening session and return to your queue")
                    .clicked()
                {
                    app.leave_room();
                    app.toast("Left listening session");
                }
            });
        }
    });
    ui.add_space(if page { 8.0 } else { 4.0 });
    let Some(room) = app.together.snapshot() else {
        return;
    };
    if room.queue.is_empty() {
        ui.label(
            egui::RichText::new("Use a song’s menu to add it to the session queue.")
                .font(theme::regular(13.0))
                .color(p.secondary),
        );
    }
    for entry in &room.queue {
        let track = entry.song.track();
        ui.horizontal(|ui| {
            ui.set_min_height(theme::ROW_HEIGHT);
            ui.add_space(theme::ROW_LEFT_INSET);
            let (rect, _) =
                ui.allocate_exact_size(egui::Vec2::splat(theme::ART_SIZE), egui::Sense::hover());
            let art = crate::ui::art_texture(
                &mut app.images,
                &app.backend,
                ui.ctx(),
                track.thumbnail.as_ref(),
                theme::ART_SIZE,
            );
            widgets::paint_art(ui, &p, rect, art, ArtShape::Square);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                avatar(
                    ui,
                    &p,
                    &mut app.images,
                    &app.backend,
                    &entry.by,
                    &format!("Added by {}", entry.by.name),
                );
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&entry.song.title)
                                .font(theme::medium(theme::ROW_TITLE_PT))
                                .color(p.text),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&entry.song.title);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&entry.song.artist)
                                .font(theme::regular(12.0))
                                .color(p.secondary),
                        )
                        .truncate(),
                    );
                });
            });
        });
    }
}

fn next_up(app: &mut App, ui: &mut egui::Ui, upcoming: &[usize]) {
    let undo = app
        .queue_undo
        .as_ref()
        .is_some_and(|(stamp, _, _)| *stamp == app.q.edit_stamp());
    crate::ui::queue_edit::prepare(ui, app.q.edit_stamp(), upcoming);
    crate::ui::queue_edit::controls(ui, &app.palette, undo, ui.available_width() < 400.0);
    let list_top = ui.cursor().top();
    let pitch = theme::ROW_HEIGHT + ui.spacing().item_spacing.y;
    let signed_in = app.signed_in();
    let clicked = {
        let App {
            palette,
            images,
            backend,
            stats,
            liked,
            now,
            q,
            radio,
            library,
            ..
        } = app;
        spec_rows(
            ui,
            palette,
            images,
            backend,
            stats,
            liked,
            upcoming.len(),
            |i| {
                let index = upcoming[i];
                let t = &q.tracks()[index];
                // No album names: this is what happens next, not where each song is
                // from.
                let subtitle = track_subtitle_no_album(t);
                RowSpec {
                    number: None,
                    rank: t.rank.as_deref(),
                    trend: t.trend,
                    title: &t.title,
                    album_id: t.album_id.as_deref(),
                    subtitle,
                    duration: t.duration,
                    // The subtitle's tail slot: the algorithm's tracks read "Radio" where
                    // the listener's own keep their play count.
                    // Manual moves can interleave chosen songs and recommendations.
                    plays: if radio
                        .as_ref()
                        .is_some_and(|r| !r.chosen.contains(&t.video_id))
                    {
                        Some("Radio")
                    } else {
                        t.plays.as_deref()
                    },
                    explicit: t.explicit,
                    active: is_active(now, &t.video_id),
                    playing: is_active(now, &t.video_id) && !now.paused,
                    badge: None,
                    // Square: the queue shows the cover, not a cropped 16:9 still.
                    shape: ArtShape::Square,
                    hide_art: false,
                    thumbnail: t.thumbnail.as_ref(),
                    menu: Some(widgets::RowMenu {
                        track: t,
                        playlist_id: None,
                        playlists: &library.playlists.items,
                        signed_in,
                        queue_index: Some(index),
                    }),
                    like: t.like,
                    // The queue is a list of what happens next, not a place to
                    // rate things: a column of thumbs repeated down it is noise.
                    // Rating a queued song is one click away on the player bar.
                    show_rating: false,
                    quiet_like: false,
                    add_to: None,
                }
            },
        )
    };

    if super::song_drag::payload(ui.ctx()).is_some_and(|d| !d.queue) {
        let rect = egui::Rect::from_min_max(
            egui::pos2(ui.min_rect().left(), list_top),
            egui::pos2(
                ui.max_rect().right(),
                list_top + upcoming.len() as f32 * pitch,
            ),
        )
        .intersect(ui.clip_rect());
        if let Some(pos) = ui
            .input(|i| i.pointer.interact_pos())
            .filter(|p| rect.contains(*p))
        {
            let boundary = (((pos.y - list_top) / pitch) + 0.5).floor().max(0.0) as usize;
            let boundary = boundary.min(upcoming.len());
            let y = list_top + boundary as f32 * pitch;
            ui.painter().hline(
                rect.x_range(),
                y,
                egui::Stroke::new(2.0, app.palette.accent),
            );
            if ui.input(|i| i.pointer.any_released())
                && let Some(drag) = super::song_drag::take(ui.ctx())
            {
                app.insert_queue_tracks(drag.tracks, boundary);
            }
        }
    }
    crate::ui::queue_edit::drop_target(ui, &app.palette, list_top, pitch, upcoming.len());
    if let Some(action) = crate::ui::queue_edit::take_action(ui.ctx()) {
        app.edit_queue(action);
        return;
    }
    // Forwarded before the click: a menu choice arrives with no row click, and the
    // `None` arm below returns early.
    if let Some(action) = clicked.action {
        app.row_action(action);
    }
    let i = match clicked.click {
        Some(ListClick::Row(i)) => upcoming[i],
        Some(ListClick::Link(route)) => {
            app.navigate(route);
            return;
        }
        None => return,
    };
    // Jumping sets `current`; the `Playing` event's `prefetch_next` follows, same as
    // every other way of moving through the queue. Clicking the row already playing just
    // pauses/resumes it (see `play_index`) — nothing new to show, so the scroll position
    // is left alone rather than jumping to the top under you.
    let already_here = app.q.current() == Some(i);
    app.play_from_queue(i);
    if !already_here {
        app.queue_scroll_reset = true;
    }
}

fn edit_controls(app: &mut App, ui: &mut egui::Ui, upcoming: &[usize], compact: bool) {
    let undo = app
        .queue_undo
        .as_ref()
        .is_some_and(|(stamp, _, _)| *stamp == app.q.edit_stamp());
    crate::ui::queue_edit::prepare(ui, app.q.edit_stamp(), upcoming);
    crate::ui::queue_edit::controls(ui, &app.palette, undo, compact);
    if let Some(action) = crate::ui::queue_edit::take_action(ui.ctx()) {
        app.edit_queue(action);
    }
}

fn external_drop(app: &mut App, ui: &mut egui::Ui, rect: egui::Rect) {
    if !super::song_drag::payload(ui.ctx()).is_some_and(|d| !d.queue) {
        return;
    }
    let target = egui::Rect::from_min_max(
        egui::pos2(rect.left(), (rect.bottom() - 48.0).max(rect.top())),
        rect.right_bottom(),
    );
    ui.painter()
        .rect_filled(target, crate::theme::RADIUS, app.palette.surface);
    ui.painter().text(
        target.center(),
        egui::Align2::CENTER_CENTER,
        "Drop to add to queue",
        theme::medium(13.0),
        app.palette.accent,
    );
    if let Some(tracks) = super::song_drag::target(ui, &app.palette, target, true) {
        app.row_action(super::RowAction::PageQueue(tracks));
    }
}
