//! The queue: what is playing, and what follows.
//!
//! Reuses `spec_rows` so virtualisation, links and art budgets apply unchanged.
//!
//! No rating thumbs here (`show_rating: false`). This is a list of what happens next, and a
//! column of controls repeated down it is noise; rating a queued song is one right-click away.

use crate::app::App;
use crate::theme;
use crate::ui::{ListClick, RowSpec, centered, is_active, spec_rows, track_subtitle_no_album};
use crate::widgets::{self, ArtShape};

pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    // The card's own rect, before the list consumes the ui: the corner repaint at the end
    // needs its rounded edges. Taken before the empty queue returns, so that card has its edge
    // too. Not in the fullscreen drawer, which is no card (see the notches below).
    let card = widgets::card_rect(ui.available_rect_before_wrap(), theme::CARD_INSET);
    let _edge = (!app.fs.open).then(|| widgets::CardEdge::new(ui, card, &app.palette));
    // What plays after the track playing, in the order it will: the rest of the list, or the
    // shuffle's order, which is drawn when shuffle goes on and then holds.
    let upcoming = app
        .q
        .upcoming(app.repeat, app.radio.as_ref().map(|r| &r.chosen));
    if upcoming.is_empty() {
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
        now_playing(app, ui);
        section(app, ui, 8.0, "Next up");
        let signed_in = app.signed_in();
        // Where the algorithm's tracks begin, for the "Radio" tail those rows carry. Computed
        // before the destructure below, which borrows `app`'s fields.
        let radio_tail = app.q.tail_start(app.radio.as_ref().map(|r| &r.chosen));
        let clicked = {
            let App {
                palette,
                images,
                backend,
                stats,
                liked,
                now,
                q,
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
                        plays: if radio_tail.is_some_and(|tail| index >= tail) {
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

/// The panel's own title, a hair right of the rows below it by eye.
fn header(app: &App, ui: &mut egui::Ui, label: &str) {
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(label)
                .font(theme::semibold(12.0))
                .color(app.palette.secondary),
        );
    });
    ui.add_space(4.0);
}

/// The track the transport is on, above the list of what follows it.
fn now_playing(app: &mut App, ui: &mut egui::Ui) {
    let Some(track) = app.q.current_track().cloned() else {
        return;
    };
    header(app, ui, "Now playing");
    // Namespaced under its own id: this row and "Next up"'s first row both start their own
    // `spec_rows` call at index 0, so without this their title/artist links resolve to the
    // exact same widget id (`ui.id()` is the same parent for both — they're sequential calls on
    // the same `Ui`, not separately salted children) and neither one's link reliably works.
    let signed_in = app.signed_in();
    let clicked = ui
        .push_id("now-playing", |ui| {
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
        .inner;
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
