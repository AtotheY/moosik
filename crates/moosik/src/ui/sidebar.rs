//! The left rail: where you can go, and what you have saved.
//!
//! It runs the whole left side of the window now — top edge to player bar — because the shell
//! claims it before the top bar rather than after a full-width titlebar. Settings has moved to
//! the top right, so nothing is pinned to the bottom except an audio failure, and that only when
//! there is one.
//!
//! Only the library list scrolls. Scrolling the whole rail meant the nav items drifted off the
//! top as soon as you had more than a handful of playlists, and the version before that put a
//! 220px box around them — a scrollbar inside a scrollbar showing five of thirty-one.

use egui::{Rect, Sense, vec2};

use crate::app::{App, Route};
use crate::chrome;
use crate::icons::Icon;
use crate::settings::LibraryItem;
use crate::theme;
use crate::ui::{ROW_ART_PT, art_texture, virtual_rows};
use crate::widgets::{self, ArtShape};

use super::organizer::Drag;

pub fn sidebar(app: &mut App, ui: &mut egui::Ui) {
    // The card's own rect, taken before any panel claims space: the corner repaint at the end
    // needs its rounded edges.
    let card = widgets::card_rect(ui.available_rect_before_wrap(), theme::CARD_INSET);
    let _edge = widgets::CardEdge::new(ui, card, &app.palette);
    widgets::clip_card_content(ui, card, theme::CARD_INSET_X, &app.palette);
    // Folded down to the icon rail: icons and covers only. Decided from the width as it is drawn,
    // so a sidebar being dragged narrower turns into the rail as it passes the snap point, and
    // lets go as one.
    let rail = theme::sidebar_is_rail(card.width() + theme::GUTTER);
    // Claimed first, so the scrolling middle gets whatever is left rather than pushing it off.
    if app.player_error.is_some() {
        egui::Panel::bottom("sidebar-footer")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| audio_warning(app, ui, rail));
    }

    app.sidebar_rail = rail;
    if app.traffic_lights_shown && !app.lights_over_rail() {
        // macOS: the traffic lights sit in the card's top-left corner, centred on the top bar
        // (`App::place_traffic_lights`). The strip they sit in drags the window like a title
        // bar, and the nav starts below it, level with the top of the page card. Fullscreen
        // hides the lights, and the nav takes the corner back.
        let band = Rect::from_min_max(
            card.left_top(),
            egui::pos2(card.right(), theme::TITLEBAR_HEIGHT),
        );
        chrome::drag_surface(ui, band, app.decorated, "sidebar");
        ui.add_space((theme::TITLEBAR_HEIGHT - ui.cursor().top()).max(8.0));
    } else {
        ui.add_space(8.0);
    }
    let providers = app.available_providers();
    let (discovery, discovery_label) = if providers.contains(&crate::model::Provider::YouTubeMusic)
    {
        (Route::Explore, "Explore")
    } else {
        (Route::NewReleases, "New releases")
    };
    for (route, icon, label) in [
        (Route::Home, Icon::House, "Home"),
        // Explore rather than Search: searching happens in the field at the top of the window,
        // and the nav slot is better spent on the page YouTube puts there.
        (discovery, Icon::Compass, discovery_label),
        (Route::Library, Icon::Library, "Library"),
        (Route::Queue, Icon::ListMusic, "Queue"),
        (Route::History, Icon::History, "Listening history"),
        (Route::Listening, Icon::TrendingUp, "Listening stats"),
    ] {
        if route == Route::NewReleases && !providers.contains(&crate::model::Provider::Spotify) {
            continue;
        }
        let current = app.current_route();
        let selected = current == route || route == Route::Explore && current == Route::NewReleases;
        let response = widgets::nav_item(ui, &app.palette, icon, label, selected, rail);
        if route == Route::Library
            && let Some(action) = super::organizer::root_drop(
                ui,
                &app.palette,
                &response,
                &app.settings.playlist_layout,
            )
        {
            super::organizer::apply(app, action);
        }
        if route == Route::Queue
            && let Some(tracks) = super::song_drag::target(
                ui,
                &app.palette,
                response.rect,
                super::song_drag::payload(ui.ctx()).is_some_and(|d| !d.queue),
            )
        {
            app.row_action(super::RowAction::PageQueue(tracks));
        }
        if response.clicked() {
            app.navigate(route);
        }
    }

    if app.listening.error.is_some() {
        ui.colored_label(app.palette.danger, "Stats need attention");
    }
    playlists(app, ui, rail);
    // The card's rounded corners, over everything the rail drew: nav rows and playlist rows
    // both run to the card's top and bottom edges now.
    widgets::paint_corner_notches(ui, card, theme::CARD_INSET_X, &app.palette);
}

/// Only shown when playback could not start: libmpv never came up, or an attempt stalled until
/// its retries ran out. The rail does not reserve space for a message that almost never appears.
fn audio_warning(app: &mut App, ui: &mut egui::Ui, rail: bool) {
    let Some(err) = app.player_error.clone() else {
        return;
    };
    if rail {
        // No room for the message: a warning sign, with the message on hover.
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            let (rect, response) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
            Icon::CircleAlert
                .image(app.palette.danger, 22.0)
                .paint_at(ui, Rect::from_center_size(rect.center(), vec2(22.0, 22.0)));
            widgets::tooltip_right(response, format!("Audio unavailable: {err}"));
        });
        ui.add_space(8.0);
        return;
    }
    ui.add_space(10.0);
    ui.label(
        egui::RichText::new("AUDIO UNAVAILABLE")
            .font(theme::semibold(10.0))
            .color(app.palette.danger),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(err)
            .font(theme::regular(11.0))
            .color(app.palette.secondary),
    );
    ui.add_space(10.0);
}

/// Your playlists, and a way to make another.
///
/// No "Library" header and no Playlists/Albums/Artists chips: the rail said Library twice — once
/// as a nav item and once over chips that duplicated the page that item opens — and the chips now
/// live on the Library page where they belong. No divider either; the card's own edge is the
/// separation, which is what the divider was doing badly.
fn playlists(app: &mut App, ui: &mut egui::Ui, rail: bool) {
    ui.add_space(14.0);
    let p = app.palette;
    let plus = if rail {
        ui.vertical_centered(|ui| widgets::icon_button(ui, &p, Icon::Plus, 18.0, false))
            .inner
    } else {
        ui.horizontal(|ui| {
            ui.set_min_height(32.0);
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Playlists")
                    .font(theme::semibold(13.0))
                    .color(p.secondary),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                widgets::icon_button(ui, &p, Icon::Plus, 18.0, false)
            })
            .inner
        })
        .inner
    }
    .on_hover_text("Create a playlist or folder");
    egui::Popup::menu(&plus)
        .frame(widgets::menu_frame(ui, &p))
        .width(206.0)
        .show(|ui| {
            if widgets::menu_item(ui, &p, Icon::ListMusic, "New playlist", None).clicked() {
                app.new_private_playlist(vec![], None);
                ui.close();
            }
            if widgets::menu_item(ui, &p, Icon::Folder, "New folder", None).clicked() {
                app.organizer.new_folder(None);
                ui.close();
            }
            if app.signed_in() {
                widgets::menu_divider(ui, &p);
                if widgets::menu_item(ui, &p, Icon::ListPlus, "New YouTube playlist", None)
                    .clicked()
                {
                    app.new_playlist = Some(String::new());
                    app.new_playlist_kind = crate::app::NewPlaylistKind::YouTube;
                    ui.close();
                }
            }
        });
    ui.add_space(4.0);
    rail_list(app, ui, rail);
}

pub(crate) fn new_playlist_popup(app: &mut App, ctx: &egui::Context) {
    use crate::app::NewPlaylistKind;
    let key = egui::Id::new("playlist-naming-open");
    let Some(mut name) = app.new_playlist.take() else {
        ctx.data_mut(|d| d.remove::<bool>(key));
        return;
    };
    let focus = ctx.data_mut(|d| d.get_temp::<bool>(key).is_none());
    ctx.data_mut(|d| d.insert_temp(key, true));
    let heading = match app.new_playlist_kind {
        NewPlaylistKind::YouTube => "New YouTube playlist",
        NewPlaylistKind::Private { rule: Some(_), .. } => "New smart playlist",
        _ => "New playlist",
    };
    let p = app.palette;
    let modal = widgets::naming_modal("playlist-name", &p).show(ctx, |ui| {
        ui.set_width((ctx.content_rect().width() - 72.0).clamp(160.0, 320.0));
        ui.label(
            egui::RichText::new(heading)
                .font(theme::semibold(18.0))
                .color(p.text),
        );
        ui.add_space(10.0);
        let entered = widgets::name_field(ui, &p, &mut name, "Playlist name", focus);
        widgets::name_actions(ui, &p, "Create", !name.trim().is_empty(), entered)
    });
    let (submit, cancel) = modal.inner;
    if submit {
        let title = name.trim().to_owned();
        match &app.new_playlist_kind {
            NewPlaylistKind::Private {
                tracks,
                rule,
                folder,
            } => {
                if !app.create_private_playlist(title, tracks.clone(), *rule, folder.clone()) {
                    app.new_playlist = Some(name);
                }
            }
            NewPlaylistKind::YouTube => app.create_playlist(title),
            NewPlaylistKind::Local { track_paths } => {
                app.create_local_playlist(title, track_paths.clone())
            }
        }
    } else if !cancel && !modal.should_close() {
        app.new_playlist = Some(name);
    }
}

/// One entry of the rail: cover, title, and what it is.
struct RailRow<'a> {
    id: &'a str,
    title: &'a str,
    detail: String,
    thumbnail: Option<&'a std::sync::Arc<str>>,
    folder: Option<bool>,
    depth: usize,
    pinned: bool,
}

#[derive(Debug, PartialEq)]
enum RailEntry {
    Playlist(usize),
    Child(usize, usize),
    Folder(String, usize),
}

/// Pinning changes position, while filing changes location. A pin remains a
/// shortcut to a filed playlist. Folders are visible even before their first member.
fn rail_entries<'a>(
    ids: impl Iterator<Item = &'a str>,
    layout: &crate::settings::PlaylistLayout,
) -> (Vec<RailEntry>, usize) {
    let ranks = layout.ranks();
    let mut children = std::collections::HashMap::<Option<String>, Vec<(usize, RailEntry)>>::new();
    let membership: std::collections::HashMap<_, _> = layout
        .folders
        .iter()
        .flat_map(|(parent, ids)| ids.iter().map(move |id| (id.as_str(), parent.as_str())))
        .collect();
    let mut entries = Vec::new();
    for id in layout.ordered_folders() {
        let rank = ranks
            .get(&LibraryItem::Folder(id.clone()))
            .copied()
            .unwrap_or(usize::MAX);
        children
            .entry(layout.parent(id).map(str::to_owned))
            .or_default()
            .push((rank, RailEntry::Folder(id.clone(), 0)));
    }
    for (i, id) in ids.enumerate() {
        let parent = membership.get(id).copied();
        let rank = ranks.get(&LibraryItem::Playlist(id.to_owned())).copied();
        if layout.pins.contains(id) {
            entries.push(RailEntry::Playlist(i));
            if parent.is_none() && rank.is_none() {
                continue;
            }
        }
        children
            .entry(parent.map(str::to_owned))
            .or_default()
            .push((rank.unwrap_or(usize::MAX), RailEntry::Playlist(i)));
    }
    for group in children.values_mut() {
        group.sort_by_key(|(rank, _)| *rank);
    }
    let pinned_count = entries.len();
    let mut stack: Vec<_> = children
        .remove(&None)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .map(|(_, row)| (row, 0))
        .collect();
    while let Some((row, depth)) = stack.pop() {
        match row {
            RailEntry::Folder(id, _) => {
                if layout.expanded_folders.contains(&id)
                    && let Some(group) = children.remove(&Some(id.clone()))
                {
                    stack.extend(group.into_iter().rev().map(|(_, row)| (row, depth + 1)));
                }
                entries.push(RailEntry::Folder(id, depth));
            }
            RailEntry::Playlist(i) if depth > 0 => entries.push(RailEntry::Child(i, depth)),
            row => entries.push(row),
        }
    }
    (entries, pinned_count)
}

fn rail_list(app: &mut App, ui: &mut egui::Ui, rail: bool) {
    let (entries, pinned_count) = rail_entries(
        app.library.playlists.iter().map(|pl| pl.id.as_str()),
        &app.settings.playlist_layout,
    );
    let count = entries.len();
    let layout = &app.settings.playlist_layout;
    let expected_children: usize = entries
        .iter()
        .filter_map(|e| match e {
            RailEntry::Folder(id, _) if layout.expanded_folders.contains(id) => {
                Some(layout.folders[id].len())
            }
            _ => None,
        })
        .sum();
    let missing_children = entries
        .iter()
        .filter(|e| matches!(e, RailEntry::Child(..)))
        .count()
        < expected_children;
    let missing_pins = pinned_count < app.settings.playlist_layout.pins.len();
    if count == 0 && !app.organizer.page_failed {
        app.load_more_library(crate::api::LibrarySection::Playlists);
    }
    let route = app.current_route();
    let liked_selected = route == Route::LikedMusic;
    let selected_id = match route {
        Route::Playlist(id) => Some(id),
        _ => None,
    };
    let selected_folder = (app.current_route() == Route::Library
        && app.rail_filter == crate::app::RailFilter::Playlists)
        .then(|| app.organizer.current_folder())
        .flatten()
        .map(str::to_owned);
    let mut folder_action = None;
    let signed_in = app.signed_in();
    let mut clicked = None;
    let mut reorder = None;
    let mut menu_action = None;
    let mut dropped = None;
    let from = widgets::painted_so_far(ui);
    let viewport_height = ui.available_height();
    let output = egui::ScrollArea::vertical()
        .id_salt("rail")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let App {
                library,
                palette,
                images,
                backend,
                playlist,
                rail_cache,
                local_playlists,
                local_tracks,
                ..
            } = app;
            let row_height = if rail {
                theme::RAIL_ROW_HEIGHT_COMPACT
            } else {
                theme::RAIL_ROW_HEIGHT
            };
            let layout = &app.settings.playlist_layout;
            let start_y = ui.cursor().top();
            if count == 0 && !rail {
                ui.weak("No playlists here");
            }
            let visible = virtual_rows(ui, row_height, count, |ui, i| {
                let depth = match &entries[i] {
                    RailEntry::Child(_, d) | RailEntry::Folder(_, d) => *d,
                    _ => 0,
                };
                let shortcut = i < pinned_count;
                let child = matches!(entries[i], RailEntry::Child(..));
                let i = match &entries[i] {
                    RailEntry::Playlist(index) | RailEntry::Child(index, _) => *index,
                    RailEntry::Folder(name, _) => {
                        let row = RailRow {
                            id: name,
                            title: layout.title(name),
                            detail: layout.description(name),
                            thumbnail: None,
                            folder: Some(layout.expanded_folders.contains(name)),
                            depth,
                            pinned: false,
                        };
                        let selected = selected_folder.as_ref() == Some(name);
                        let response = if rail {
                            rail_row_compact(ui, palette, &row, None, selected)
                        } else {
                            rail_row(ui, palette, &row, None, selected)
                        };
                        if let Some(action) = super::organizer::folder_interaction(
                            ui, palette, &response, name, layout, true,
                        ) {
                            folder_action = Some(action);
                        }
                        return;
                    }
                };
                let p = &library.playlists.items[i];
                let is_liked = crate::model::is_liked_auto(&p.id);
                let row = RailRow {
                    id: &p.id,
                    title: if is_liked {
                        super::liked_music::TITLE
                    } else {
                        &p.title
                    },
                    // The section header already says Playlists; the row says who made it,
                    // when it is someone else's list, and how much is in it when it is yours.
                    detail: if is_liked { String::new() } else { p.detail() },
                    thumbnail: if is_liked { None } else { p.thumbnail.as_ref() },
                    folder: None,
                    depth,
                    pinned: app.settings.playlist_layout.pins.contains(&p.id),
                };
                let art = art_texture(images, backend, ui.ctx(), row.thumbnail, ROW_ART_PT);
                let selected =
                    (liked_selected && is_liked) || selected_id.as_deref() == Some(row.id);
                let response = if rail {
                    rail_row_compact(ui, palette, &row, art, selected)
                } else {
                    rail_row(ui, palette, &row, art, selected)
                };
                let accepts = !is_liked
                    && p.editable
                    && super::song_drag::payload(ui.ctx()).is_some_and(|d| {
                        if p.id.starts_with("private:") {
                            true
                        } else if p.source == crate::model::PlaylistSource::Local {
                            d.tracks.iter().all(|t| t.local_path.is_some())
                        } else {
                            signed_in && d.tracks.iter().all(|t| t.local_path.is_none())
                        }
                    });
                if let Some(tracks) = super::song_drag::target(ui, palette, response.rect, accepts)
                {
                    dropped = Some((p.id.clone(), tracks));
                }
                super::organizer::remember_source(
                    ui,
                    response.rect,
                    LibraryItem::Playlist(p.id.clone()),
                    false,
                );
                if response.drag_started()
                    || (response.is_pointer_button_down_on()
                        && ui.input(|i| i.pointer.primary_pressed()))
                {
                    egui::DragAndDrop::set_payload(ui.ctx(), Drag(vec![p.id.clone()]));
                }
                if !shortcut {
                    if let (_, Some(insert)) = super::organizer::insertion_drop(
                        ui,
                        palette,
                        response.rect,
                        LibraryItem::Playlist(p.id.clone()),
                        layout,
                        true,
                        false,
                    ) {
                        folder_action = Some(insert);
                    }
                } else if let Some(drag) = response.dnd_hover_payload::<Drag>()
                    && drag.0.len() == 1
                    && let Some(id) = drag.0.first()
                    && id != &p.id
                    && layout.pins.contains(id)
                    && let Some(from) = library.playlists.iter().position(|pl| &pl.id == id)
                {
                    let y = if i < from {
                        response.rect.top()
                    } else {
                        response.rect.bottom()
                    };
                    ui.painter()
                        .hline(response.rect.x_range(), y, (2.0, palette.accent));
                    if response.dnd_release_payload::<Drag>().is_some() {
                        reorder = Some((from, i));
                    }
                }
                if response.clicked() {
                    clicked = Some(Route::Playlist(row.id.to_owned()));
                }
                // Right-click: the same ••• menu the playlist page offers. The track actions
                // (shuffle, queue, save) need the playlist's tracks, so a first right-click
                // kicks off a fetch; the menu shows those items at once and an early pick waits
                // for the songs (`tracks_pending`). A local
                // playlist never needs this — it's already fully in memory, so `local_view`
                // below is built fresh every time instead of going through `rail_cache` at all.
                let is_local = p.source == crate::model::PlaylistSource::Local;
                if response.secondary_clicked()
                    && !is_local
                    && !is_liked
                    && !rail_cache.contains_key(&p.id)
                    && playlist.as_ref().map(|pl| pl.id.as_str()) != Some(p.id.as_str())
                {
                    backend.fetch_playlist(p.id.clone());
                }
                let local_view = is_local
                    .then(|| local_playlists.iter().find(|lp| lp.id == p.id))
                    .flatten()
                    .map(|lp| crate::app::build_local_playlist(lp, local_tracks));
                let cached = local_view
                    .as_ref()
                    .or_else(|| {
                        rail_cache
                            .get(&p.id)
                            .or_else(|| playlist.as_ref().filter(|pl| pl.id == p.id))
                    })
                    .filter(|_| !is_liked);
                let tracks: &[crate::model::Track] =
                    cached.map(|pl| pl.tracks.items.as_slice()).unwrap_or(&[]);
                if let Some(action) = widgets::page_menu_with_layout(
                    ui,
                    palette,
                    &response,
                    widgets::PageMenu {
                        page_id: &p.id,
                        tracks,
                        artist_id: p.author_id.as_deref().filter(|_| !is_liked),
                        playlists: &library.playlists.items,
                        signed_in,
                        // There is no real YouTube URL for a local playlist to share.
                        share_url: if is_local || is_liked {
                            String::new()
                        } else {
                            crate::model::playlist_url(&p.id)
                        },
                        // Not here: the description only arrives with the fetch, so Info would
                        // pop into an open menu. The playlist page's own ••• offers it.
                        has_info: false,
                        manage: !is_liked && (p.editable || p.id.starts_with("private:")),
                        remove_from_library: !is_local && !is_liked && !p.editable,
                        context_only: true,
                        // A local playlist has no YouTube radio to start.
                        radio_id: (!is_local && !is_liked).then_some(p.id.as_str()),
                        save_as_new: None,
                        // The songs a right-click has only just asked for: the menu opens
                        // complete, and an item picked before they land waits for them.
                        tracks_pending: (!is_liked && cached.is_none())
                            .then_some(crate::ui::PageKind::Playlist),
                    },
                    Some((row.pinned, child || layout.is_filed(&p.id))),
                ) {
                    menu_action = Some(action);
                }
            });
            // The tail is part of the scrolling content, even at an exact fit. Short
            // libraries get a hit target all the way down to the player.
            let tail_h = (viewport_height - (ui.cursor().top() - start_y)).max(row_height * 1.6);
            let (_, tail) =
                ui.allocate_exact_size(vec2(ui.available_width(), tail_h), Sense::click());
            if let Some(action) = super::organizer::root_drop(ui, palette, &tail, layout) {
                folder_action = Some(action);
            } else if tail.clicked() {
                folder_action = Some(super::organizer::Action::Folder(None));
            }
            egui::Popup::context_menu(&tail)
                .frame(widgets::menu_frame(ui, palette))
                .show(|ui| {
                    if widgets::menu_item(ui, palette, Icon::Folder, "New folder", None).clicked() {
                        folder_action = Some(super::organizer::Action::NewFolder(None));
                        ui.close();
                    }
                });
            if super::organizer::library_drag(ui.ctx())
                && ui.input(|i| i.pointer.is_decidedly_dragging())
                && let Some(pos) = ui
                    .input(|i| i.pointer.interact_pos())
                    .filter(|p| ui.clip_rect().contains(*p))
            {
                let clip = ui.clip_rect();
                let delta = (32.0 - (pos.y - clip.top())).max(0.0)
                    - (32.0 - (clip.bottom() - pos.y)).max(0.0);
                if delta != 0.0 {
                    ui.scroll_with_delta(vec2(0.0, delta * 0.35));
                    ui.ctx().request_repaint();
                }
            }
            visible
        });
    if (missing_pins || missing_children || output.inner.end == count) && !app.organizer.page_failed
    {
        app.load_more_library(crate::api::LibrarySection::Playlists);
    }
    if let Some(action) = folder_action {
        super::organizer::apply(app, action);
    }
    widgets::fade_scroll_edges(ui, from, &output, widgets::scroll_fade(&app.settings));
    if let Some((id, tracks)) = dropped {
        app.drop_tracks_on_playlist(&id, tracks);
    }
    if let Some((from, to)) = reorder {
        app.move_playlist(from, to);
    }
    if let Some(action) = menu_action {
        app.row_action(action);
    }
    if let Some(route) = clicked {
        app.navigate(route);
    }
}

fn disclosure(ui: &egui::Ui, p: &theme::Palette, center: egui::Pos2, open: bool) {
    let icon = if open {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    icon.image(p.secondary, 14.0)
        .paint_at(ui, Rect::from_center_size(center, vec2(14.0, 14.0)));
}

fn rail_art(
    ui: &egui::Ui,
    palette: &theme::Palette,
    rect: Rect,
    row: &RailRow<'_>,
    art: Option<&egui::TextureHandle>,
) {
    if row.folder.is_some() {
        ui.painter()
            .rect_filled(rect, theme::RADIUS, palette.glass(palette.surface));
        Icon::Folder
            .image(palette.secondary, 22.0)
            .paint_at(ui, Rect::from_center_size(rect.center(), vec2(22.0, 22.0)));
    } else {
        if crate::model::is_liked_auto(row.id) {
            super::liked_music::paint_cover(ui, palette, rect);
        } else {
            widgets::paint_art(ui, palette, rect, art, ArtShape::Square);
        }
        if row.pinned {
            let center = rect.left_top() + vec2(7.0, 7.0);
            ui.painter().circle_filled(center, 7.0, palette.accent);
            Icon::Bookmark
                .image(palette.on_accent, 9.0)
                .paint_at(ui, Rect::from_center_size(center, vec2(9.0, 9.0)));
        }
    }
}

/// A row of the icon rail: the cover, centred, with the title and what it is as a tooltip. The pill
/// for the selected and the hovered row is the same as the full row's.
fn rail_row_compact(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    row: &RailRow<'_>,
    art: Option<&egui::TextureHandle>,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::RAIL_ROW_HEIGHT_COMPACT),
        Sense::click_and_drag(),
    );
    if ui.is_rect_visible(rect) {
        let t = widgets::lit(ui, &response);
        if selected || t > 0.001 {
            let fill = if selected {
                palette.glass(palette.surface_active)
            } else {
                palette.glass_state(palette.surface_hover, t)
            };
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(theme::RADIUS), fill);
        }
        let art_rect = Rect::from_center_size(
            rect.center(),
            vec2(theme::RAIL_ART_SIZE, theme::RAIL_ART_SIZE),
        );
        let art_rect = if row.depth > 0 {
            art_rect.shrink(5.0).translate(vec2(3.0, 0.0))
        } else {
            art_rect
        };
        rail_art(ui, palette, art_rect, row, art);
        if let Some(open) = row.folder {
            let c = art_rect.right_bottom() - vec2(6.0, 6.0);
            ui.painter().circle_filled(c, 7.0, palette.surface);
            disclosure(ui, palette, c, open);
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    widgets::tooltip_right(response, rail_tooltip(row.title, &row.detail))
}

/// What a rail row says on hover: its title, and under it what it is when there is something to say.
fn rail_tooltip(title: &str, detail: &str) -> String {
    if detail.is_empty() {
        title.to_owned()
    } else {
        format!("{title}\n{detail}")
    }
}

/// A rail row: cover, title, and what it is — the shape the reference uses, and the reason the
/// rail is worth 280pt instead of 240.
fn rail_row(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    row: &RailRow<'_>,
    art: Option<&egui::TextureHandle>,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::RAIL_ROW_HEIGHT),
        Sense::click_and_drag(),
    );
    if ui.is_rect_visible(rect) {
        let t = widgets::lit(ui, &response);
        if selected || t > 0.001 {
            let fill = if selected {
                palette.glass(palette.surface_active)
            } else {
                palette.glass_state(palette.surface_hover, t)
            };
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(theme::RADIUS), fill);
        }

        let indent = (row.depth as f32 * 18.0).min((rect.width() - 128.0).max(0.0));
        let inset = indent + if row.folder.is_some() { 22.0 } else { 6.0 };
        let size = if row.depth > 0 {
            34.0
        } else if row.folder.is_some() {
            32.0
        } else {
            theme::RAIL_ART_SIZE
        };
        if let Some(open) = row.folder {
            disclosure(
                ui,
                palette,
                rect.left_center() + vec2(10.0 + indent, 0.0),
                open,
            );
        }
        if row.depth > 0 {
            ui.painter().vline(
                rect.left() + indent - 8.0,
                rect.y_range(),
                (1.0, palette.surface_active),
            );
        }
        let art_rect = Rect::from_center_size(
            rect.left_center() + vec2(inset + size / 2.0, 0.0),
            vec2(size, size),
        );
        rail_art(ui, palette, art_rect, row, art);

        let x = art_rect.right() + 10.0;
        let w = (rect.right() - 8.0 - x).max(0.0);
        let title_font = theme::medium(13.0);
        crate::emoji::paint_line(
            ui,
            egui::pos2(x, rect.center().y - 9.0),
            &widgets::truncate_to(ui, row.title, title_font.clone(), w),
            title_font,
            if selected {
                palette.accent_text()
            } else {
                palette.text
            },
        );
        let detail_font = theme::regular(11.0);
        crate::emoji::paint_line(
            ui,
            egui::pos2(x, rect.center().y + 9.0),
            &widgets::truncate_to(ui, &row.detail, detail_font.clone(), w),
            detail_font,
            palette.dim,
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::rail_tooltip;

    #[test]
    fn pins_lead_and_both_folders_and_unpinned_playlists_remain_reachable() {
        use super::{RailEntry::*, rail_entries};
        use crate::settings::PlaylistLayout;
        let mut layout = PlaylistLayout::default();
        layout.pins.insert("pin".into());
        layout.folders.insert("Empty".into(), Default::default());
        layout
            .folders
            .insert("Filed".into(), ["pin".into(), "filed".into()].into());
        let (entries, count) =
            rail_entries(["ordinary", "filed", "pin", "another"].into_iter(), &layout);
        assert_eq!(count, 1);
        assert_eq!(
            entries,
            vec![
                Playlist(2),
                Folder("Empty".into(), 0),
                Folder("Filed".into(), 0),
                Playlist(0),
                Playlist(3)
            ]
        );
        let (entries, _) = rail_entries(std::iter::empty(), &layout);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn expanded_folders_follow_saved_order_and_unfiling_keeps_the_playlist() {
        use super::{RailEntry::*, rail_entries};
        use crate::settings::PlaylistLayout;
        let mut layout = PlaylistLayout::default();
        layout.move_to(&["one".into(), "two".into()].into(), Some("First"));
        layout.move_to(&Default::default(), Some("Second"));
        layout.pins.insert("one".into());
        layout.expanded_folders.insert("First".into());
        layout.insert_beside(
            &[crate::settings::LibraryItem::Folder("Second".into())],
            crate::settings::LibraryItem::Folder("First".into()),
            false,
            &[],
        );
        let entries = |l: &PlaylistLayout| rail_entries(["two", "one"].into_iter(), l).0;
        assert_eq!(
            entries(&layout),
            vec![
                Playlist(1),
                Folder("Second".into(), 0),
                Folder("First".into(), 0),
                Child(0, 1),
                Child(1, 1)
            ]
        );
        layout.move_to(&["two".into()].into(), None);
        let saved: PlaylistLayout =
            serde_json::from_str(&serde_json::to_string(&layout).unwrap()).unwrap();
        assert_eq!(
            entries(&saved),
            vec![
                Playlist(1),
                Folder("Second".into(), 0),
                Folder("First".into(), 0),
                Child(1, 1),
                Playlist(0)
            ]
        );
        layout.expanded_folders.clear();
        assert!(!entries(&layout).contains(&Child(1, 1)));
        layout.insert_beside(
            &[crate::settings::LibraryItem::Folder("Second".into())],
            crate::settings::LibraryItem::Folder("First".into()),
            true,
            &[],
        );
        assert_eq!(layout.ordered_folders(), ["First", "Second"]);
    }

    #[test]
    fn a_filed_playlist_dropped_between_folders_keeps_that_exact_slot() {
        use super::{LibraryItem, RailEntry::*, rail_entries};
        use crate::settings::PlaylistLayout;
        let mut layout = PlaylistLayout::default();
        layout.move_to(&["mix".to_owned()].into(), Some("B"));
        layout.move_to(&Default::default(), Some("A"));
        let item = LibraryItem::Playlist("mix".into());
        layout.insert_beside(
            std::slice::from_ref(&item),
            LibraryItem::Folder("B".into()),
            false,
            &["mix".into()],
        );
        let saved: PlaylistLayout =
            serde_json::from_value(serde_json::to_value(layout).unwrap()).unwrap();
        assert!(saved.folders["B"].is_empty());
        assert_eq!(
            rail_entries(["mix"].into_iter(), &saved).0,
            [Folder("A".into(), 0), Playlist(0), Folder("B".into(), 0)]
        );
        let mut nested = saved;
        let parent = nested.create_folder("Parent", None);
        nested.reparent("A", Some(&parent));
        nested.reparent("B", Some(&parent));
        nested.insert_beside(
            &[item],
            LibraryItem::Folder("A".into()),
            true,
            &["mix".into()],
        );
        assert_eq!(
            rail_entries(["mix"].into_iter(), &nested).0,
            [
                Folder(parent, 0),
                Folder("A".into(), 1),
                Child(0, 1),
                Folder("B".into(), 1)
            ]
        );
    }

    /// What the icon rail says on hover: the title, and under it what the playlist is when there
    /// is something to say; a bare title, not a title and an empty line, when there is not.
    #[test]
    fn a_rail_tooltip_is_the_title_and_the_detail_when_there_is_one() {
        assert_eq!(rail_tooltip("Gym", "12 tracks"), "Gym\n12 tracks");
        assert_eq!(rail_tooltip("Gym", ""), "Gym");
    }
}
