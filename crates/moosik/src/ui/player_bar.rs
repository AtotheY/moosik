//! The now-playing bar at the bottom: cover, title, transport, seek and volume.

use egui::{Color32, Rect, Sense, vec2};

use crate::app::{App, RepeatMode, Route};
use crate::icons::Icon;
use crate::model::{EditablePlaylists, LikeStatus, Provider, Track};
use crate::theme;
use crate::ui::RowAction;
use crate::widgets;

#[derive(Default)]
pub(crate) struct PlaylistPicker {
    track_id: Option<String>,
    choices: Option<Result<EditablePlaylists, String>>,
    liked_override: Option<bool>,
    loading: bool,
    query: String,
    focus_search: bool,
}

impl PlaylistPicker {
    pub(crate) fn can_add(&self, track_id: &str, playlist_id: &str) -> bool {
        self.choices_for(track_id).is_some_and(|choices| {
            choices
                .playlists
                .iter()
                .any(|(playlist, saved)| playlist.id == playlist_id && !saved)
        })
    }

    fn choices_for(&self, track_id: &str) -> Option<&EditablePlaylists> {
        if self.track_id.as_deref() != Some(track_id) {
            return None;
        }
        self.choices.as_ref()?.as_ref().ok()
    }

    pub(crate) fn loaded(&mut self, track_id: &str, result: Result<EditablePlaylists, String>) {
        if self.track_id.as_deref() == Some(track_id) {
            self.loading = false;
            self.choices = Some(result);
        }
    }

    pub(crate) fn note_saved(&mut self, track_id: &str, playlist_id: &str) {
        if self.track_id.as_deref() == Some(track_id)
            && let Some(Ok(choices)) = &mut self.choices
            && let Some((_, member)) = choices
                .playlists
                .iter_mut()
                .find(|(playlist, _)| playlist.id == playlist_id)
        {
            *member = true;
        }
    }

    pub(crate) fn invalidate(&mut self, track_id: &str) {
        if self.track_id.as_deref() == Some(track_id) {
            *self = Self::default();
        }
    }

    pub(crate) fn note_liked(&mut self, track_id: &str, liked: bool) {
        if self.track_id.as_deref() == Some(track_id) {
            self.liked_override = Some(liked);
        }
    }

    fn is_liked(&self, track: &Track) -> bool {
        if self.track_id.as_deref() == Some(&track.video_id)
            && let Some(liked) = self.liked_override
        {
            return liked;
        }
        (if track.is_spotify() {
            track.in_library
        } else {
            track.like == LikeStatus::Like
        }) || self
            .choices_for(&track.video_id)
            .is_some_and(|choices| choices.liked == Some(true))
    }

    fn contains_track(&self, track: &Track) -> bool {
        self.is_liked(track)
            || self
                .choices_for(&track.video_id)
                .is_some_and(|choices| choices.playlists.iter().any(|(_, saved)| *saved))
    }
}

fn playlist_popup(app: &mut App, ui: &egui::Ui, button: &egui::Response) {
    let Some(track) = app.now.track.as_ref() else {
        return;
    };
    let provider = track.provider();
    let picker = &mut app.playlist_picker;
    if button.clicked() {
        picker.query.clear();
        if picker.track_id.as_deref() != Some(track.video_id.as_str()) {
            picker.track_id = Some(track.video_id.clone());
            picker.choices = None;
            picker.liked_override = None;
            picker.loading = false;
        }
        if track.is_spotify()
            && let Some(liked) = app.liked_music.spotify_liked(&track.video_id)
        {
            picker.note_liked(&track.video_id, liked);
        }
        if !picker.loading && picker.choices.as_ref().is_none_or(Result::is_err) {
            picker.loading = true;
            app.backend
                .fetch_editable_playlists(provider, track.video_id.clone());
        }
    }
    let palette = app.palette;
    let popup = egui::Popup::menu(button)
        .align(egui::RectAlign::TOP_START)
        .gap(8.0)
        .frame(widgets::menu_frame(ui, &palette).inner_margin(12))
        .width(300.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    if button.clicked() {
        picker.focus_search = !popup.is_open();
    }
    let choice = popup
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(
                egui::RichText::new("Add to playlist")
                    .font(theme::semibold(18.0))
                    .color(palette.text),
            );
            ui.add_space(4.0);
            let opening = picker.focus_search;
            let search = playlist_search(ui, &palette, &mut picker.query, opening);
            if !ui.is_sizing_pass() {
                picker.focus_search = false;
            }
            ui.add_space(4.0);

            let query = picker.query.trim().to_lowercase();
            let mut action = None;
            let liked = picker.is_liked(track);
            let show_liked = "liked songs".contains(&query);
            if show_liked {
                let save = playlist_choice(ui, &palette, "Liked Songs", liked, true, |ui, rect| {
                    super::liked_music::paint_cover(ui, &palette, rect);
                })
                .on_hover_text(if liked {
                    "Remove from Liked Songs"
                } else {
                    "Add to Liked Songs"
                });
                if save.clicked() {
                    action = Some(if track.is_spotify() {
                        RowAction::SetSpotifyLiked {
                            track: track.clone(),
                            liked: !liked,
                        }
                    } else {
                        RowAction::Rate {
                            video_id: track.video_id.clone(),
                            status: if liked {
                                LikeStatus::Indifferent
                            } else {
                                LikeStatus::Like
                            },
                        }
                    });
                }
            }
            match picker.choices_for(&track.video_id) {
                Some(choices) => {
                    let mut matches = choices
                        .playlists
                        .iter()
                        .filter(|(playlist, _)| {
                            query.is_empty() || playlist.title.to_lowercase().contains(&query)
                        })
                        .peekable();
                    if matches.peek().is_some() {
                        if show_liked {
                            widgets::menu_divider(ui, &palette);
                        }
                        ui.label(
                            egui::RichText::new("Your playlists")
                                .font(theme::semibold(12.0))
                                .color(palette.secondary),
                        );
                        let list_height =
                            (ui.ctx().content_rect().height() - theme::PLAYER_BAR_HEIGHT - 140.0)
                                .clamp(44.0, 284.0);
                        let mut list = egui::ScrollArea::vertical()
                            .max_height(list_height)
                            .min_scrolled_height(list_height)
                            .auto_shrink([false, true]);
                        if opening || search.changed() {
                            list = list.vertical_scroll_offset(0.0);
                        }
                        list.show(ui, |ui| {
                            for (playlist, saved) in matches {
                                let add = ui
                                    .push_id(&playlist.id, |ui| {
                                        playlist_choice(
                                            ui,
                                            &palette,
                                            &playlist.title,
                                            *saved,
                                            !saved,
                                            |ui, rect| {
                                                let art = super::art_texture(
                                                    &mut app.images,
                                                    &app.backend,
                                                    ui.ctx(),
                                                    playlist.thumbnail.as_ref(),
                                                    rect.width(),
                                                );
                                                widgets::paint_art(
                                                    ui,
                                                    &palette,
                                                    rect,
                                                    art,
                                                    widgets::ArtShape::Square,
                                                );
                                            },
                                        )
                                    })
                                    .inner;
                                if add.clicked() {
                                    action = Some(RowAction::SaveToPlaylist {
                                        playlist_id: playlist.id.clone(),
                                        video_id: track.video_id.clone(),
                                    });
                                }
                            }
                        });
                    } else if query.is_empty() {
                        playlist_status(ui, &palette, "No editable playlists");
                    } else if !show_liked {
                        playlist_status(ui, &palette, "No matching playlists");
                    }
                }
                _ if picker.loading => {
                    playlist_status(ui, &palette, "Loading playlists…");
                }
                _ => {
                    playlist_status(ui, &palette, "Could not load playlists");
                    if widgets::menu_item(ui, &palette, Icon::RefreshCw, "Retry", None).clicked() {
                        picker.loading = true;
                        app.backend
                            .fetch_editable_playlists(provider, track.video_id.clone());
                    }
                }
            }
            if action.is_some() {
                ui.close();
            }
            action
        })
        .and_then(|result| result.inner);
    if let Some(action) = choice {
        app.row_action(action);
    }
}

fn playlist_search(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    query: &mut String,
    focus: bool,
) -> egui::Response {
    let field = widgets::input_frame(palette)
        .fill(palette.panel)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(Icon::Search.image(palette.secondary, 15.0));
                ui.add(
                    egui::TextEdit::singleline(query)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .font(theme::regular(13.0))
                        .hint_text(egui::RichText::new("Find a playlist").color(palette.secondary))
                        .desired_width(ui.available_width()),
                )
            })
            .inner
        });
    if focus && !ui.is_sizing_pass() {
        field.inner.request_focus();
    }
    if field.inner.has_focus() {
        ui.painter().rect_stroke(
            field.response.rect,
            theme::RADIUS,
            egui::Stroke::new(1.0, palette.secondary),
            egui::StrokeKind::Inside,
        );
    }
    field.inner
}

fn playlist_choice(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    title: &str,
    saved: bool,
    enabled: bool,
    paint_cover: impl FnOnce(&egui::Ui, Rect),
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), sense);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, saved, title)
    });
    if ui.is_rect_visible(rect) {
        let t = if enabled {
            widgets::lit(ui, &response)
        } else {
            0.0
        };
        if widgets::held(&response) || t > 0.0 {
            let fill = if widgets::held(&response) {
                palette.surface_active
            } else {
                palette.surface_hover.gamma_multiply(t)
            };
            ui.painter().rect_filled(rect, theme::RADIUS, fill);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect,
                theme::RADIUS,
                egui::Stroke::new(1.0, palette.secondary),
                egui::StrokeKind::Inside,
            );
        }
        let offset = widgets::press_offset(&response);
        let cover = Rect::from_center_size(
            rect.left_center() + vec2(22.0, 0.0) + offset,
            vec2(32.0, 32.0),
        );
        paint_cover(ui, cover);
        let label = widgets::truncate_to(
            ui,
            title,
            theme::medium(14.0),
            (rect.width() - 84.0).max(0.0),
        );
        crate::emoji::paint_line(
            ui,
            rect.left_center() + vec2(48.0, 0.0) + offset,
            &label,
            theme::medium(14.0),
            palette.text,
        );
        let (icon, color) = if saved {
            (Icon::Check, palette.text)
        } else {
            (Icon::Plus, palette.secondary)
        };
        icon.image(color, 16.0).paint_at(
            ui,
            Rect::from_center_size(
                rect.right_center() - vec2(16.0, 0.0) + offset,
                vec2(16.0, 16.0),
            ),
        );
        if enabled && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    response
}

fn playlist_status(ui: &mut egui::Ui, palette: &theme::Palette, message: &str) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(message)
            .font(theme::regular(13.0))
            .color(palette.secondary),
    );
    ui.add_space(4.0);
}

/// The player bar's cover is drawn 56pt across and is always on screen.
const BAR_ART_PT: f32 = 56.0;
/// How wide the title and credits field is allowed to grow before the rating column stops
/// following it. Wide enough for a full title at 13pt, narrow enough that the thumbs stay
/// beside the credits rather than out towards the transport.
const FIELD_MAX_W: f32 = 320.0;
/// The transport's width: shuffle, previous, play, next, repeat. Centred on the window, and
/// shared with the title cap so the two never overlap.
const TRANSPORT_W: f32 = 250.0;
/// The play button's diameter, and the transport row's height.
const PLAY_DIAMETER: f32 = 40.0;
/// An icon button's box at the transport's 16 pt glyphs (`widgets::icon_button` pads by 14).
const TRANSPORT_ICON_BOX: f32 = 16.0 + 14.0;
/// Where the transport row and the seek bar sit, from the bar's vertical centre. Together they
/// span from the play button's top to the time labels' bottom, and these offsets centre that
/// span on the bar — the line the cover and the right-hand controls are centred on — rather
/// than leaving it 6 pt high, as the old −12/+16 did.
const TRANSPORT_DY: f32 = -6.0;
const SEEK_DY: f32 = 22.0;
/// A pair of 36pt targets, grouped like YouTube Music's player rating control.
const RATING_PILL_W: f32 = 72.0;
const RATING_PILL_H: f32 = 32.0;
const CONTROL_GAP: f32 = 8.0;
/// Right inset, slider, gap and mute button.
const COMPACT_CLUSTER_W: f32 = 14.0 + 76.0 + CONTROL_GAP + 30.0;
/// The full cluster also has Queue and Lyrics. A video adds one more button.
const CONTROL_CLUSTER_W: f32 = COMPACT_CLUSTER_W + 2.0 * (30.0 + CONTROL_GAP);
/// Below this much room for the title and credits, use the compact transport and top seek line.
const FULL_FIELD_MIN: f32 = 80.0;

fn control_cluster_w(ratings: bool, panels: bool, video: bool, volume_track: bool) -> f32 {
    let volume = if volume_track {
        COMPACT_CLUSTER_W
    } else {
        14.0 + 30.0
    };
    volume
        + if panels {
            CONTROL_CLUSTER_W - COMPACT_CLUSTER_W
        } else {
            0.0
        }
        + if video { 30.0 + CONTROL_GAP } else { 0.0 }
        + if ratings {
            RATING_PILL_W + CONTROL_GAP
        } else {
            0.0
        }
}

/// Keep the ratings and transport reachable at the minimum window width. The volume slider
/// yields to its mute button only when keeping it would squeeze out the song's credits.
fn compact_volume_track(width: f32, save_w: f32, row_w: f32, ratings: bool) -> bool {
    let title_x = 14.0 + BAR_ART_PT + 12.0;
    width
        - control_cluster_w(ratings, false, false, true)
        - CONTROL_GAP
        - row_w
        - save_w
        - CONTROL_GAP
        - title_x
        >= FULL_FIELD_MIN
}

fn rating_pill(ui: &mut egui::Ui, palette: &theme::Palette, track: &Track) -> Option<RowAction> {
    let (rect, _) = ui.allocate_exact_size(vec2(RATING_PILL_W, RATING_PILL_H), Sense::hover());
    let radius = egui::CornerRadius::same(16);
    ui.painter()
        .rect_filled(rect, radius, palette.glass(palette.surface));
    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0, palette.outline.gamma_multiply(0.55)),
        egui::StrokeKind::Inside,
    );
    ui.painter().vline(
        rect.center().x,
        rect.top() + 8.0..=rect.bottom() - 8.0,
        egui::Stroke::new(1.0, palette.outline),
    );
    let mut action = None;
    for (index, (up, status)) in [(true, LikeStatus::Like), (false, LikeStatus::Dislike)]
        .into_iter()
        .enumerate()
    {
        let hit = Rect::from_min_size(
            rect.min + vec2(index as f32 * RATING_PILL_W / 2.0, 0.0),
            vec2(RATING_PILL_W / 2.0, RATING_PILL_H),
        );
        let selected = track.like == status;
        let label = crate::ui::tips::rate(up, selected);
        let response = widgets::rate_button(
            ui,
            palette,
            hit,
            ui.id().with(("np-rating", &track.video_id, up)),
            up,
            selected,
            true,
        )
        .on_hover_text(label);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, label)
        });
        if response.has_focus() {
            ui.painter().rect_stroke(
                hit.shrink(2.0),
                egui::CornerRadius::same(14),
                egui::Stroke::new(1.0, palette.accent),
                egui::StrokeKind::Inside,
            );
        }
        if response.clicked() {
            action = Some(RowAction::Rate {
                video_id: track.video_id.clone(),
                status: track.like.toggled(status),
            });
        }
    }
    action
}

/// The seek bar's width: grows with the window from the transport's own width up to 420pt,
/// and never so wide that the duration label hanging right of it reaches the control cluster.
///
/// It grows from the transport's width rather than as a flat third of the window: at a
/// mid-size window a third was wide enough that the elapsed label, which the now-playing text
/// has to clear, cut "The Weeknd • Kiss Land" down to "Kiss Lan" with room to spare in the
/// middle.
fn seek_width(width: f32, cluster_left: f32, total_w: f32, center_x: f32) -> f32 {
    // The label is painted 10pt past the bar's right end; the 6pt is the gap it must leave.
    let room = cluster_left - 6.0 - 10.0 - total_w - center_x;
    (width * 0.5 - 250.0)
        .clamp(TRANSPORT_W, 420.0)
        .min(2.0 * room.max(0.0))
}

/// How wide the title and credits field is in the full layout of a bar `width` wide: from the
/// cover to the transport, or to the elapsed label hanging left of the seek bar, whichever is
/// nearer, less the thumbs. The same clearances `player_bar` lays the full bar out with.
fn full_field_w(width: f32, thumbs_w: f32, total_w: f32, cluster_w: f32) -> f32 {
    let center = width / 2.0;
    let bar_w = seek_width(width, width - cluster_w, total_w, center);
    let title_x = 14.0 + BAR_ART_PT + 12.0;
    (center - TRANSPORT_W / 2.0 - 8.0).min(center - bar_w / 2.0 - 10.0 - total_w - 22.0)
        - thumbs_w
        - title_x
}

/// Where playback is and how long the song runs, for the seek bar. A restored, not-yet-loaded
/// track still shows its saved position and length: mpv has no state until play, so `resume`
/// stands in, and scrubbing updates it rather than the player.
pub(crate) fn clock(app: &App) -> (f64, f64) {
    let state = app.player_state;
    let restored = (!app.playback_started)
        .then(|| app.q.resume_here())
        .flatten();
    let position = restored.unwrap_or(state.position);
    let duration = if state.duration > 0.0 {
        state.duration
    } else {
        app.now
            .track
            .as_ref()
            .and_then(|t| t.duration)
            .map(|d| d as f64)
            .unwrap_or(0.0)
    };
    (position, duration)
}

pub fn player_bar(app: &mut App, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let p = app.palette;

    // No divider: the bar sits on the gutter with the cards above it, so the gap is the edge.

    let state = app.player_state;

    let art = Rect::from_min_size(
        rect.left_top() + vec2(14.0, (rect.height() - 56.0) / 2.0),
        vec2(56.0, 56.0),
    );
    let bar_art = crate::ui::art_texture(
        &mut app.images,
        &app.backend,
        ui.ctx(),
        app.now.track.as_ref().and_then(|t| t.thumbnail.as_ref()),
        BAR_ART_PT,
    );
    widgets::paint_art(ui, &p, art, bar_art, widgets::ArtShape::Square);
    let collection = app.listening.collection.as_ref();
    let sleeve = app
        .now
        .track
        .as_ref()
        .and_then(|track| collection?.sleeves.get(&track.video_id));
    if let Some(sleeve) = sleeve {
        super::keepsakes::sleeve(ui, art, sleeve.completed);
    }
    let artist_pass = app.now.track.as_ref().and_then(|track| {
        let artist = track.artists.first()?;
        let key = crate::listening::collection::artist_key(&artist.name, artist.id.as_deref());
        let pass = collection?.passes.get(&key)?;
        (!pass.stamps.is_empty()).then_some(key)
    });
    // A failure replaces the credits; otherwise the credits are the same linked pieces a row
    // draws, which is what makes *every* artist here clickable rather than only the first.
    // Loading keeps them: the ring on the play button says it, and the artist is what you look
    // for while you wait.
    let (title, status) = match (&app.now.track, &app.now.error) {
        (Some(t), Some(e)) => (t.title.clone(), Some((e.clone(), p.danger))),
        (Some(t), None) => (t.title.clone(), None),
        // Nothing playing, nothing said — not even a dash.
        (None, _) => (String::new(), None),
    };

    let title_x = art.right() + 12.0;
    let provider = app.now.track.as_ref().map(Track::provider);
    let save_picker = match provider {
        Some(Provider::YouTubeMusic) => app.signed_in(),
        Some(Provider::Spotify) => crate::spotify::is_signed_in(),
        _ => false,
    };
    let spotify_remove = save_picker
        && provider == Some(Provider::Spotify)
        && app.playing_spotify_playlist_item().is_some();
    let thumbs_w = if spotify_remove {
        72.0
    } else if save_picker {
        36.0
    } else {
        0.0
    };
    // The seek bar is wider than the transport, and its elapsed-time label hangs left of
    // the bar at subtitle height — the title has to clear the label, not just the buttons,
    // or long titles sit underneath it. The total stands in for the elapsed (at least as
    // wide, and stable while time advances), and the thumbs clear it too. The 24pt floor
    // keeps a degenerate window an ellipsis rather than the full title over everything.
    let total_w = widgets::text_width(
        ui,
        &widgets::fmt_duration(state.duration as u64),
        &theme::regular(11.0),
    );
    let ratings = provider == Some(Provider::YouTubeMusic) && app.signed_in();
    let screen_w = ui.ctx().content_rect().width();
    let panels = theme::right_panel_fits(screen_w);
    let video = app.now.track.as_ref().is_some_and(|track| track.wide);
    let full_cluster_w = control_cluster_w(ratings, panels, video, true);
    let compact = full_field_w(rect.width(), thumbs_w, total_w, full_cluster_w) < FULL_FIELD_MIN
        || rect.width() - full_cluster_w < rect.width() / 2.0 + TRANSPORT_W / 2.0 + CONTROL_GAP;
    // The transport row's own width — 30 pt icon buttons around the 40 pt play button and the
    // gaps between: four icons in full, two compact (no shuffle or repeat).
    let icons = if compact { 2.0 } else { 4.0 };
    let row_w = icons * TRANSPORT_ICON_BOX + PLAY_DIAMETER + icons * ui.spacing().item_spacing.x;
    let volume_track = !compact || compact_volume_track(rect.width(), thumbs_w, row_w, ratings);
    let cluster_w = if compact {
        control_cluster_w(ratings, false, false, volume_track)
    } else {
        full_cluster_w
    };
    // Full, the transport is centred on the window; compact, it sits against the volume, which
    // leaves the title all the room to its left.
    let (transport_x, transport_dy, transport_start) = if compact {
        let x = rect.right() - cluster_w - 8.0 - row_w / 2.0;
        (x, 0.0, x - row_w / 2.0)
    } else {
        let x = rect.center().x;
        (x, TRANSPORT_DY, x - TRANSPORT_W / 2.0)
    };
    // Compact, the seek bar is a line along the bar's top from the cover's left edge to the
    // volume's right, with no labels for the title to clear.
    let bar = if compact {
        Rect::from_min_size(
            rect.left_top() + vec2(14.0, 4.0),
            vec2(rect.width() - 28.0, 4.0),
        )
    } else {
        let bar_w = seek_width(
            rect.width(),
            rect.right() - cluster_w,
            total_w,
            rect.center().x,
        );
        Rect::from_center_size(
            egui::pos2(rect.center().x, rect.center().y + SEEK_DY),
            vec2(bar_w, 4.0),
        )
    };
    // How far right the title's field may reach before the elapsed label hanging left of the bar.
    let label_left = if compact {
        f32::INFINITY
    } else {
        bar.left() - 10.0 - total_w
    };
    // The field hugs its own content — the wider of the title and the credits — so the
    // rating column rides next to the text instead of at a fixed 320pt from the title, which
    // on a short song left the thumbs drifting in space and is the whole reason this line is
    // not a clamp. The credits are never shortened to get there: the field is at least the
    // credits' own width whenever the clearances allow. Past the cap (or squeezed by the
    // transport) the title and credits scroll on hover instead, which is what the marquee is
    // for; the 24pt floor keeps a degenerate window an ellipsis.
    let source = app
        .now
        .track
        .as_ref()
        .and_then(|track| widgets::visible_source_badge(ui, track.source_name()));
    let content_w = app
        .now
        .track
        .as_ref()
        .map(|t| {
            widgets::subtitle_width(ui, &crate::ui::track_subtitle(t), None)
                + if artist_pass.is_some() { 20.0 } else { 0.0 }
        })
        .unwrap_or(0.0)
        .max(
            widgets::text_width(ui, &title, &theme::medium(13.0))
                + if source.is_some() {
                    widgets::SOURCE_BADGE_WIDTH
                } else {
                    0.0
                },
        );
    let text_w = (content_w + 8.0)
        .clamp(24.0, FIELD_MAX_W)
        .min((transport_start - thumbs_w - 8.0 - title_x).max(0.0))
        .min((label_left - 22.0 - thumbs_w - title_x).max(0.0))
        .max(24.0);
    // The column, then the field the two lines share: title above, credits below.
    let thumbs_x = (title_x + text_w + 10.0)
        .min(transport_start - thumbs_w)
        .min(label_left - 4.0 - thumbs_w);
    let field_w = (thumbs_x - 10.0 - title_x).max(24.0);
    // The title links to its album, and — with the cover — is the only place the song's
    // menu opens. `marquee_line` hands back its own click-sensing viewport when the title
    // overflows; a title that fits gets a hit rect the width of its text, so neither the
    // blank field nor the credits answer the pointer.
    let title_font = theme::medium(13.0);
    let source_w = if field_w >= 64.0
        && let Some(source) = source
    {
        widgets::source_badge(
            ui,
            &p,
            egui::pos2(title_x, rect.center().y - 9.0),
            source,
            ui.id().with("np-source"),
        );
        widgets::SOURCE_BADGE_WIDTH
    } else {
        0.0
    };
    let title_field_w = field_w - source_w;
    let title_pos = egui::pos2(title_x + source_w, rect.center().y - 9.0);
    let title_w = widgets::text_width(ui, &title, &title_font).min(title_field_w);
    let title_rect = Rect::from_min_size(
        egui::pos2(title_pos.x, title_pos.y - widgets::MARQUEE_HIT_H / 2.0),
        vec2(title_w, widgets::MARQUEE_HIT_H),
    );
    // A long title scrolls on hover inside the field rather than pushing anything away.
    let title_hit = widgets::marquee_line(
        ui,
        title_pos,
        title_field_w,
        &title,
        title_font,
        p.text,
        "np-title",
        Sense::click(),
        false,
    )
    .unwrap_or_else(|| ui.interact(title_rect, ui.id().with("np-title-hit"), Sense::click()));
    let art_hit = ui.interact(art, ui.id().with("np-art-hit"), Sense::click());
    if let Some(sleeve) = sleeve.filter(|s| s.completed >= 5) {
        art_hit.clone().on_hover_text(format!(
            "Your copy · {} full listens\nFirst heard {}",
            sleeve.completed, sleeve.first_day
        ));
    }
    // Hovering the playing song's cover offers the fullscreen view: the cover dims and an expand
    // glyph appears on it. A primary click opens it; the right-click menu is unchanged.
    if app.now.track.is_some() {
        let t = ui
            .ctx()
            .animate_bool_with_time(art_hit.id.with("hover"), art_hit.hovered(), 0.12);
        if t > 0.0 {
            ui.painter().rect_filled(
                art,
                widgets::ArtShape::Square.radius(art),
                Color32::from_black_alpha((140.0 * t) as u8),
            );
            Icon::Maximize2
                .image(Color32::WHITE.gamma_multiply(t), 20.0)
                .paint_at(ui, Rect::from_center_size(art.center(), vec2(20.0, 20.0)));
        }
        if art_hit.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if art_hit.clicked() {
            let ctx = ui.ctx().clone();
            app.fs_enter(&ctx);
        }
    }
    if app.now.track.as_ref().is_some_and(|t| t.album_id.is_some()) && title_hit.hovered() {
        // The same hover treatment a row's artist link gets: a pointing hand and a hairline
        // under the text.
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().hline(
            title_pos.x..=(title_pos.x + title_w),
            title_pos.y + 7.0,
            egui::Stroke::new(1.0, p.text),
        );
    }
    let menu_response = app
        .now
        .track
        .as_ref()
        .map(|_| art_hit.union(title_hit.clone()));

    // The credits, through the same painter a row uses: one hit rect per artist, so every
    // artist is clickable and the album is a link too. The viewport is the field, not a
    // width derived from the title — a short song name no longer starves its own credits.
    // Scrolls on hover like the title.
    let baseline = egui::pos2(title_x, rect.center().y + 9.0);
    let mut navigate = None;
    match &status {
        Some((text, colour)) => {
            ui.painter().text(
                baseline,
                egui::Align2::LEFT_CENTER,
                widgets::truncate_to(ui, text, theme::regular(12.0), field_w),
                theme::regular(12.0),
                *colour,
            );
        }
        None => {
            if let Some(track) = app.now.track.as_ref()
                && field_w > 4.0
            {
                let subtitle = crate::ui::track_subtitle(track);
                let mut credit_pos = baseline;
                let mut credit_width = field_w;
                if let Some(key) = &artist_pass
                    && let Some(pass) = app
                        .listening
                        .collection
                        .as_ref()
                        .and_then(|c| c.passes.get(key))
                    && field_w >= 46.0
                {
                    let after_name = track.artists.len() == 1 && track.album.is_none();
                    let width = widgets::subtitle_width(ui, &subtitle, None).min(field_w - 20.);
                    let at = if after_name {
                        baseline + vec2(width + 2., 0.)
                    } else {
                        baseline
                    };
                    super::keepsakes::player_pass(ui, &p, at, pass, key);
                    if !after_name {
                        credit_pos.x += 20.;
                    }
                    credit_width -= 20.;
                }
                if let Some(link) = widgets::paint_subtitle_marquee(
                    ui,
                    &p,
                    credit_pos,
                    credit_width,
                    &subtitle,
                    None,
                    "np-subtitle",
                    false,
                    theme::regular(12.0),
                    p.secondary,
                ) {
                    navigate = Some(crate::ui::link_route(link));
                }
            }
        }
    }
    if let Some(route) = navigate {
        let route = match route {
            Route::Artist(id) => app.combined_artist_route(&id).unwrap_or(Route::Artist(id)),
            route => route,
        };
        app.navigate(route);
    }
    // A primary click on the title opens its album or single; the menu is the right-click.
    if title_hit.clicked()
        && let Some(id) = app.now.track.as_ref().and_then(|t| t.album_id.clone())
    {
        app.navigate(Route::Album(id));
    }

    let mut remove = None;
    let mut picker_button = None;
    if save_picker && let Some(track) = app.now.track.as_ref() {
        let thumbs = Rect::from_min_size(
            egui::pos2(thumbs_x, rect.center().y - 16.0),
            vec2(thumbs_w, 32.0),
        );
        let mut tui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(thumbs)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        tui.spacing_mut().item_spacing.x = 4.0;
        if spotify_remove
            && widgets::spotify_remove_button(&mut tui, &p).clicked()
            && let Some((playlist_id, uid)) = app.playing_spotify_playlist_item()
        {
            remove = Some(RowAction::RemoveFromPlaylist {
                playlist_id: playlist_id.to_owned(),
                video_id: track.video_id.clone(),
                set_video_id: uid.to_owned(),
            });
        }
        picker_button = Some(
            tui.push_id(&track.video_id, |ui| {
                let saved = app.playlist_picker.contains_track(track);
                widgets::playlist_save_button(ui, &p, saved, true)
            })
            .inner,
        );
    }
    if let Some(action) = remove {
        app.row_action(action);
    }
    if let Some(button) = picker_button {
        playlist_popup(app, ui, &button);
    }

    // Transport, centred on the window so it does not shift with the track title: shuffle and
    // repeat flank previous/play/next, the way the reference lays them out. The box is exactly
    // the row's own width (`row_w`) because the row packs from the left: a wider box left its
    // spare room on the right and pushed every button ~30 pt left of the seek bar under it.
    let controls = Rect::from_center_size(
        egui::pos2(transport_x, rect.center().y + transport_dy),
        vec2(row_w, PLAY_DIAMETER),
    );
    let mut cui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(controls)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );

    if !compact
        && widgets::icon_button(&mut cui, &p, Icon::Shuffle, 16.0, app.q.is_shuffled())
            .on_hover_text(crate::ui::tips::shuffle(app.q.is_shuffled()))
            .clicked()
    {
        app.toggle_shuffle();
    }
    let mut skip_delta = 0isize;
    if widgets::icon_button(&mut cui, &p, Icon::SkipBackFilled, 16.0, false)
        .on_hover_text(crate::ui::tips::PREVIOUS)
        .clicked()
    {
        skip_delta = -1;
    }
    let playing = app.playback_started && !state.paused;
    // While the song is still loading the button already shows pause — you asked it to play —
    // with the loading ring turning around its rim.
    let loading = widgets::track_loading(cui.ctx());
    let play = widgets::play_button(&mut cui, &p, playing || loading, PLAY_DIAMETER)
        .on_hover_text(crate::ui::tips::play(playing || loading));
    if loading {
        widgets::paint_spinner(
            &cui,
            play.rect.center(),
            play.rect.width() / 2.0 + 3.0,
            2.0,
            p.accent,
        );
    }
    if play.clicked() {
        app.toggle_playback();
    }
    if widgets::icon_button(&mut cui, &p, Icon::SkipForwardFilled, 16.0, false)
        .on_hover_text(crate::ui::tips::NEXT)
        .clicked()
    {
        skip_delta = 1;
    }
    let repeat_icon = if app.repeat == RepeatMode::One {
        Icon::Repeat1
    } else {
        Icon::Repeat
    };
    if !compact
        && widgets::icon_button(
            &mut cui,
            &p,
            repeat_icon,
            16.0,
            app.repeat != RepeatMode::Off,
        )
        .on_hover_text(crate::ui::tips::repeat(app.repeat))
        .clicked()
    {
        app.cycle_repeat();
    }
    if skip_delta != 0 {
        app.skip(skip_delta);
    }

    // Seek bar, click or drag to scrub. `bar` is shared with the title cap above, so
    // the elapsed-time label the bar paints can never sit under the title.
    let (position, duration) = clock(app);
    let fraction = if duration > 0.0 {
        (position / duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if let Some(seek_to) =
        widgets::seek_bar(ui, &p, bar, fraction as f32, duration, position, !compact)
    {
        app.seek_to(seek_to);
    }

    // Volume. The child Ui spans the whole cluster, not just the slider: it packs from the
    // right, and a narrower rect would only hide the room the buttons already use.
    let vol_rect = Rect::from_min_max(
        egui::pos2(rect.right() - cluster_w, rect.top()),
        rect.right_bottom(),
    );
    let mut vui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(vol_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    // Explicit gaps match the cluster measurements, including the volume widget's own gap.
    vui.spacing_mut().item_spacing.x = CONTROL_GAP;
    // `vui` packs right-to-left, so whatever is added first ends up closest to the window's
    // edge: the volume slider goes first to sit rightmost, with Queue and Lyrics to its left.
    vui.add_space(14.0);
    // The user's volume, not `player.volume()`: the Audio normaliser folds the track's gain into
    // the live value.
    let volume = if app.muted { 0.0 } else { app.last_volume };
    if volume_track {
        if let Some(v) = widgets::volume_slider(&mut vui, &p, volume, app.last_volume) {
            app.set_user_volume(v);
        }
    } else {
        let icon = if app.muted || volume <= 0.5 {
            Icon::VolumeX
        } else {
            Icon::Volume2
        };
        if widgets::icon_button(&mut vui, &p, icon, 16.0, false)
            .on_hover_text(crate::ui::tips::mute(app.muted || volume <= 0.5))
            .clicked()
        {
            app.toggle_mute();
        }
    }
    // Queue and Lyrics only where their panel can be shown: a narrower window hides the panel
    // (`theme::right_panel_fits`), and a button that toggled nothing visible would read as
    // broken. The fullscreen player carries both at any width.
    if !compact && panels {
        if widgets::icon_button(&mut vui, &p, Icon::ListMusic, 16.0, app.upnext_open)
            .on_hover_text(crate::ui::tips::QUEUE)
            .clicked()
        {
            app.upnext_open = !app.upnext_open;
        }
        if widgets::icon_button(&mut vui, &p, Icon::MicVocal, 16.0, app.lyrics_open)
            .on_hover_text(crate::ui::tips::LYRICS)
            .clicked()
        {
            app.toggle_lyrics();
        }
    }
    // The video view, further left still, and only when the track actually has one.
    if !compact
        && video
        && widgets::icon_button(&mut vui, &p, Icon::Video, 16.0, app.video_open)
            .on_hover_text(crate::ui::tips::video(app.video_open))
            .clicked()
    {
        app.toggle_video();
    }

    if ratings
        && let Some(track) = app.now.track.as_ref()
        && let Some(action) = rating_pill(&mut vui, &p, track)
    {
        app.row_action(action);
    }

    // The now-playing song's menu, last: painting is done, and the choices route through the
    // same handler every row's menu uses.
    if let Some(menu) = menu_response
        && let Some(track) = app.now.track.as_ref()
    {
        let action = widgets::row_menu(
            ui,
            &p,
            &menu,
            widgets::RowMenu {
                track,
                playlist_id: None,
                playlists: &app.library.playlists.items,
                signed_in: app.signed_in(),
                queue_index: None,
            },
        );
        if let Some(action) = action {
            app.row_action(action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_picker_distinguishes_likes_from_library_saves() {
        let mut track = crate::listening::Metadata {
            id: "video".to_owned(),
            ..Default::default()
        }
        .to_track();
        let mut picker = PlaylistPicker {
            track_id: Some(track.video_id.clone()),
            ..Default::default()
        };
        track.in_library = true;
        assert!(!picker.contains_track(&track));
        track.like = LikeStatus::Like;
        assert!(picker.contains_track(&track));

        picker.loaded(
            &track.video_id,
            Ok(EditablePlaylists {
                liked: Some(true),
                playlists: Vec::new(),
            }),
        );
        track.like = LikeStatus::Indifferent;
        picker.note_liked(&track.video_id, false);
        assert!(!picker.contains_track(&track));
    }

    /// A wide window keeps the bar's designed width; a narrow one shrinks it so the duration
    /// label stops short of the control cluster, where the video toggle used to sit on it.
    #[test]
    fn the_seek_bar_shrinks_before_its_label_reaches_the_controls() {
        // 1100pt window: half less 250 is 300, under the cap and the cluster.
        let wide = seek_width(1100.0, 1100.0 - 14.0 - CONTROL_CLUSTER_W, 30.0, 550.0);
        assert!((wide - 300.0).abs() < 0.01, "{wide}");
        // A maximised 1920pt window reaches the cap.
        let full = seek_width(1920.0, 1920.0 - 14.0 - CONTROL_CLUSTER_W, 30.0, 960.0);
        assert!((full - 420.0).abs() < 0.01, "{full}");

        // 760pt window with a long duration: the cluster is the binding limit.
        let narrow = seek_width(760.0, 760.0 - 14.0 - CONTROL_CLUSTER_W, 45.0, 380.0);
        assert!(narrow < TRANSPORT_W, "{narrow}");
        // The label's right edge — half the bar, the 10pt offset and its own width — clears
        // the cluster with the 6pt gap intact.
        let label_right = 380.0 + narrow / 2.0 + 10.0 + 45.0;
        let cluster_left = 760.0 - 14.0 - CONTROL_CLUSTER_W;
        assert!(label_right <= cluster_left - 6.0 + 0.01, "{label_right}");

        // A window narrower than the controls can afford: the bar collapses rather than
        // painting under them.
        assert_eq!(
            seek_width(400.0, 400.0 - 14.0 - CONTROL_CLUSTER_W, 30.0, 200.0),
            0.0
        );
    }

    /// The bar goes compact where the full layout would squeeze the song to a sliver — half a
    /// portrait screen, and the old 760 floor — and stays full in an ordinary window.
    #[test]
    fn a_narrow_bar_goes_compact_and_a_roomy_one_stays_full() {
        // The bar is the window less the gutter and its 6 pt inset on each side.
        let bar = |window: f32| window - 2.0 * (theme::GUTTER + 6.0);
        for window in [540.0, 760.0] {
            assert!(
                full_field_w(bar(window), 72.0, 30.0, CONTROL_CLUSTER_W) < FULL_FIELD_MIN,
                "{window}"
            );
        }
        for window in [900.0, 1100.0, 1920.0] {
            assert!(
                full_field_w(bar(window), 72.0, 30.0, CONTROL_CLUSTER_W) >= FULL_FIELD_MIN,
                "{window}"
            );
        }
        // Signed out, no thumbs: the title keeps their room and the bar stays full for longer.
        assert!(full_field_w(bar(760.0), 0.0, 30.0, CONTROL_CLUSTER_W) >= FULL_FIELD_MIN);
    }

    #[test]
    fn rating_pill_toggles_and_switches_both_ratings_for_the_playing_song() {
        let ctx = egui::Context::default();
        let palette = theme::Palette::dark();
        let mut track = crate::listening::Metadata {
            id: "playing-video".to_owned(),
            ..Default::default()
        }
        .to_track();
        let mut time = 0.0;
        for initial in [
            LikeStatus::Indifferent,
            LikeStatus::Like,
            LikeStatus::Dislike,
        ] {
            track.like = initial;
            for (index, pressed) in [LikeStatus::Like, LikeStatus::Dislike]
                .into_iter()
                .enumerate()
            {
                let mut action = None;
                let pill_rect = std::cell::Cell::new(Rect::NOTHING);
                let mut frame = |events| {
                    time += 0.05;
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                egui::Pos2::ZERO,
                                vec2(400.0, 120.0),
                            )),
                            time: Some(time),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            pill_rect.set(Rect::from_min_size(
                                ui.next_widget_position(),
                                vec2(RATING_PILL_W, RATING_PILL_H),
                            ));
                            if let Some(choice) = rating_pill(ui, &palette, &track) {
                                action = Some(choice);
                            }
                        },
                    );
                    output.textures_delta.clear();
                };
                frame(Vec::new());
                let pos = pill_rect.get().left_center()
                    + vec2((index as f32 + 0.5) * RATING_PILL_W / 2.0, 0.0);
                frame(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                frame(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }]);
                assert!(
                    matches!(action,
                        Some(RowAction::Rate { ref video_id, status })
                            if video_id == "playing-video" && status == initial.toggled(pressed)
                    ),
                    "initial={initial:?}, pressed={pressed:?}"
                );
            }
        }
    }

    #[test]
    fn ratings_leave_room_for_transport_and_song_at_the_minimum_window_width() {
        let width = theme::WINDOW_MIN_W - 2.0 * (theme::GUTTER + 6.0);
        let row_w = 2.0 * TRANSPORT_ICON_BOX + PLAY_DIAMETER + 2.0 * CONTROL_GAP;
        let save_w = 36.0;
        assert!(!compact_volume_track(width, save_w, row_w, true));
        assert!(compact_volume_track(760.0, save_w, row_w, true));
        let cluster_w = control_cluster_w(true, false, false, false);
        let transport_start = width - cluster_w - CONTROL_GAP - row_w;
        let title_x = 14.0 + BAR_ART_PT + 12.0;
        assert!(transport_start - save_w - CONTROL_GAP - title_x >= FULL_FIELD_MIN);

        // Even with a video and both panels, seek labels clear the pill at every full width.
        for window in [900.0, 1100.0, 1512.0, 1920.0] {
            let width = window - 2.0 * (theme::GUTTER + 6.0);
            let cluster_left = width - control_cluster_w(true, true, true, true);
            let bar_w = seek_width(width, cluster_left, 45.0, width / 2.0);
            assert!(width / 2.0 + bar_w / 2.0 + 10.0 + 45.0 <= cluster_left - 6.0);
        }
    }
}
