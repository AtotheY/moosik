//! Device-local playlist organization. All selection and drag payloads use playlist IDs.
use std::collections::{BTreeSet, HashMap};

use crate::app::{App, Route};
use crate::icons::Icon;
use crate::settings::{LibraryItem, PlaylistLayout};
use crate::{theme, widgets};

#[derive(Default)]
pub(crate) struct View {
    pub(crate) page_failed: bool,
    folder: Option<String>,
    query: String,
    selecting: bool,
    selected: BTreeSet<String>,
    selected_display_order: Vec<String>,
    anchor: Option<String>,
    dialog: Option<Dialog>,
}

enum Dialog {
    Move(BTreeSet<String>),
    Name {
        parent: Option<String>,
        previous: Option<String>,
        name: String,
        ids: BTreeSet<String>,
        focus: bool,
    },
}

#[derive(Clone)]
pub(super) struct Drag(pub(super) Vec<String>);

impl Drag {
    fn from_grid(id: &str, selected: &BTreeSet<String>, display_order: &[&str]) -> Self {
        if selected.contains(id) {
            Self(
                display_order
                    .iter()
                    .filter(|id| selected.contains(**id))
                    .map(|id| (*id).to_owned())
                    .collect(),
            )
        } else {
            Self(vec![id.to_owned()])
        }
    }
}

pub(super) enum Action {
    Open(String),
    Play(String),
    Folder(Option<String>),
    Toggle(String),
    Insert(Vec<LibraryItem>, LibraryItem, bool),
    Nest(String, Option<String>),
    NewFolder(Option<String>),
    NewPlaylist(String),
    MoveOut(BTreeSet<String>),
    Move(BTreeSet<String>, Option<String>),
    Pin(BTreeSet<String>, bool),
    Rename(String),
    Shuffle(String),
    Remove(String),
    Pick(BTreeSet<String>),
}

pub(crate) fn file_created(app: &mut App, id: String, folder: String) {
    apply(app, Action::Move([id].into(), Some(folder)));
}

pub(crate) fn unfile_one(app: &mut App, id: String) {
    apply(app, Action::MoveOut([id].into()));
}

pub(crate) fn pin_one(app: &mut App, id: String, pin: bool) {
    apply(app, Action::Pin(BTreeSet::from([id]), pin));
}

impl View {
    pub(crate) fn new_folder(&mut self, parent: Option<String>) {
        self.dialog = Some(Dialog::Name {
            parent,
            previous: None,
            name: String::new(),
            ids: BTreeSet::new(),
            focus: true,
        });
    }

    pub(crate) fn current_folder(&self) -> Option<&str> {
        self.folder.as_deref()
    }

    pub(crate) fn move_one(&mut self, id: String) {
        self.dialog = Some(Dialog::Move(BTreeSet::from([id])));
    }

    fn finish_selection(&mut self) {
        self.selecting = false;
        self.selected.clear();
        self.selected_display_order.clear();
        self.anchor = None;
    }

    fn select(&mut self, id: &str, visible: &[&str], extend: bool) {
        self.selecting = true;
        if extend
            && let Some(from) = visible
                .iter()
                .position(|s| Some(*s) == self.anchor.as_deref())
            && let Some(to) = visible.iter().position(|s| *s == id)
        {
            self.selected.extend(
                visible[from.min(to)..=from.max(to)]
                    .iter()
                    .map(|s| s.to_string()),
            );
        } else if !self.selected.remove(id) {
            self.selected.insert(id.to_owned());
        }
        self.anchor = Some(id.to_owned());
    }
}

fn commit(app: &mut App, change: impl FnOnce(&mut PlaylistLayout)) -> bool {
    let mut settings = app.settings.clone();
    change(&mut settings.playlist_layout);
    if let Err(error) = crate::settings::save(&settings) {
        tracing::warn!(%error, "saving playlist folders");
        app.toast_error("Couldn't save your library layout");
        return false;
    }
    app.settings.playlist_layout = settings.playlist_layout;
    true
}

pub(super) fn apply(app: &mut App, action: Action) {
    match action {
        Action::Open(id) => app.navigate(Route::Playlist(id)),
        Action::Play(id) => app.play_playlist(id),
        Action::Folder(name) => {
            app.organizer.folder = name;
            app.organizer.query.clear();
            app.organizer.finish_selection();
            app.rail_filter = crate::app::RailFilter::Playlists;
            app.navigate(Route::Library);
            app.scroll_to_top = true;
        }
        Action::Toggle(name) => {
            commit(app, |layout| {
                if !layout.expanded_folders.remove(&name) {
                    layout.expanded_folders.insert(name);
                }
            });
        }
        Action::NewPlaylist(parent) => app.new_private_playlist(vec![], Some(parent)),
        Action::NewFolder(parent) => app.organizer.new_folder(parent),
        Action::Nest(id, parent) => {
            commit(app, |layout| {
                layout.reparent(&id, parent.as_deref());
            });
        }
        Action::MoveOut(ids) => {
            if commit(app, |layout| layout.move_out(&ids)) {
                app.toast("Moved out of folder");
                app.organizer.finish_selection();
            }
        }
        Action::Insert(items, target, after) => {
            let playlists = app
                .library
                .playlists
                .iter()
                .map(|p| p.id.clone())
                .collect::<Vec<_>>();
            if commit(app, |layout| {
                layout.insert_beside(&items, target, after, &playlists)
            }) {
                app.organizer.finish_selection();
            }
        }
        Action::Pick(ids) => app.organizer.dialog = Some(Dialog::Move(ids)),
        Action::Shuffle(name) => app.shuffle_folder(&name),
        Action::Rename(previous) => {
            app.organizer.dialog = Some(Dialog::Name {
                parent: app
                    .settings
                    .playlist_layout
                    .parent(&previous)
                    .map(str::to_owned),
                name: app.settings.playlist_layout.title(&previous).to_owned(),
                previous: Some(previous),
                ids: Default::default(),
                focus: true,
            })
        }
        Action::Remove(name) => {
            let parent = app
                .settings
                .playlist_layout
                .parent(&name)
                .map(str::to_owned);
            if commit(app, |layout| layout.ungroup(&name)) {
                app.toast("Folder removed · contents kept");
                if app.organizer.folder.as_ref() == Some(&name) {
                    app.organizer.folder = parent;
                }
            }
        }
        Action::Move(ids, folder) => {
            if commit(app, |layout| layout.move_to(&ids, folder.as_deref())) {
                app.toast(&format!(
                    "Moved to {}",
                    folder
                        .as_deref()
                        .map(|id| app.settings.playlist_layout.title(id))
                        .unwrap_or("Library")
                ));
                app.organizer.finish_selection();
            }
        }
        Action::Pin(ids, pin) => {
            if commit(app, |layout| {
                if pin {
                    layout.pins.extend(ids);
                } else {
                    layout.pins.retain(|id| !ids.contains(id));
                }
            }) {
                app.organizer.finish_selection();
                app.toast(if pin {
                    "Pinned to sidebar"
                } else {
                    "Unpinned from sidebar"
                });
            }
        }
    }
}

#[derive(Clone)]
struct FolderDrag(String);

/// A fast gesture can deliver mouse-down, travel and mouse-up in one paint.
/// Resolve its source against the layout that the user actually pressed, before
/// any destination (which may paint earlier than the source) handles the release.
#[derive(Clone)]
struct DragOrigin {
    rect: egui::Rect,
    layer: egui::LayerId,
    item: LibraryItem,
    grid: bool,
}

fn origins_key() -> egui::Id {
    egui::Id::new("library-drag-origins")
}

pub(super) fn remember_source(ui: &egui::Ui, r: egui::Rect, item: LibraryItem, grid: bool) {
    let rect = ui.clip_rect().intersect(r);
    if rect.is_positive() {
        ui.ctx().data_mut(|d| {
            d.get_temp_mut_or_default::<Vec<DragOrigin>>(origins_key())
                .push(DragOrigin {
                    rect,
                    layer: ui.layer_id(),
                    item,
                    grid,
                })
        });
    }
}

pub(crate) fn begin_drag_frame(ctx: &egui::Context, view: &View) {
    let press = ctx.input(|i| {
        i.raw.events.iter().find_map(|event| match event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            } => Some(*pos),
            _ => None,
        })
    });
    let layer = press.and_then(|p| ctx.layer_id_at(p));
    let source = ctx.data_mut(|d| {
        let origins = d.get_temp_mut_or_default::<Vec<DragOrigin>>(origins_key());
        let source = press.and_then(|p| {
            origins
                .iter()
                .rev()
                .find(|src| Some(src.layer) == layer && src.rect.contains(p))
                .cloned()
        });
        origins.clear();
        source
    });
    if let Some(src) = source {
        match src.item {
            LibraryItem::Folder(id) => egui::DragAndDrop::set_payload(ctx, FolderDrag(id)),
            LibraryItem::Playlist(id) => {
                let ids = if src.grid && view.selected.contains(&id) {
                    view.selected_display_order.clone()
                } else {
                    vec![id]
                };
                egui::DragAndDrop::set_payload(ctx, Drag(ids));
            }
        }
    }
}

pub(super) fn library_drag(ctx: &egui::Context) -> bool {
    egui::DragAndDrop::has_payload_of_type::<FolderDrag>(ctx)
        || egui::DragAndDrop::has_payload_of_type::<Drag>(ctx)
}

/// The literal gutter belongs to the following row. The adjacent edge bands make
/// the target forgiving, while the folder's center remains available for nesting.
pub(super) fn insertion_drop(
    ui: &egui::Ui,
    p: &theme::Palette,
    r: egui::Rect,
    target: LibraryItem,
    layout: &PlaylistLayout,
    vertical: bool,
    folder: bool,
) -> (bool, Option<Action>) {
    if !library_drag(ui.ctx()) {
        return (false, None);
    }
    let mut hit = r;
    let spacing = ui.spacing().item_spacing;
    if vertical {
        hit.min.y -= spacing.y;
    } else {
        hit.min.x -= spacing.x;
    }
    if !ui.rect_contains_pointer(hit) {
        return (false, None);
    }
    let Some(pos) = ui.input(|i| i.pointer.interact_pos()) else {
        return (false, None);
    };
    let fraction = if vertical {
        (pos.y - r.top()) / r.height()
    } else {
        (pos.x - r.left()) / r.width()
    };
    if folder && (0.22..=0.78).contains(&fraction) {
        return (false, None);
    }
    let after = fraction > 0.5;
    let items = if let Some(drag) = egui::DragAndDrop::payload::<FolderDrag>(ui.ctx()) {
        if !layout.can_place(&drag.0, layout.item_parent(&target)) {
            return (true, None);
        }
        vec![LibraryItem::Folder(drag.0.clone())]
    } else if let Some(drag) = egui::DragAndDrop::payload::<Drag>(ui.ctx()) {
        drag.0.iter().cloned().map(LibraryItem::Playlist).collect()
    } else {
        return (false, None);
    };
    if items.contains(&target) {
        return (true, None);
    }
    let stroke = egui::Stroke::new(2.0, p.accent);
    if vertical {
        ui.painter().hline(
            r.x_range(),
            if after {
                r.bottom() + spacing.y / 2.0
            } else {
                r.top() - spacing.y / 2.0
            },
            stroke,
        );
    } else {
        ui.painter().vline(
            if after {
                r.right() + spacing.x / 2.0
            } else {
                r.left() - spacing.x / 2.0
            },
            r.y_range(),
            stroke,
        );
    }
    let action = ui.input(|i| i.pointer.primary_released()).then(|| {
        egui::DragAndDrop::clear_payload(ui.ctx());
        Action::Insert(items, target, after)
    });
    (true, action)
}

fn drop_inside(
    ui: &egui::Ui,
    p: &theme::Palette,
    response: &egui::Response,
    parent: Option<&str>,
    layout: &PlaylistLayout,
) -> Option<Action> {
    // egui can assign the nearest widget to a pointer in the gutter. The folder
    // body must never consume a release that belongs to the insertion line.
    if !ui.rect_contains_pointer(response.rect) {
        return None;
    }
    let action = if let Some(drag) = response.dnd_hover_payload::<FolderDrag>() {
        if layout.parent(&drag.0) == parent || !layout.can_place(&drag.0, parent) {
            return None;
        }
        Action::Nest(drag.0.clone(), parent.map(str::to_owned))
    } else {
        let drag = response.dnd_hover_payload::<Drag>()?;
        if drag.0.iter().all(|id| layout.playlist_parent(id) == parent) {
            return None;
        }
        Action::Move(drag.0.iter().cloned().collect(), parent.map(str::to_owned))
    };
    ui.painter().rect_stroke(
        response.rect.shrink(1.0),
        theme::RADIUS,
        (2.0, p.accent),
        egui::StrokeKind::Inside,
    );
    if ui.input(|i| i.pointer.primary_released()) {
        egui::DragAndDrop::clear_payload(ui.ctx());
        Some(action)
    } else {
        None
    }
}

pub(super) fn root_drop(
    ui: &egui::Ui,
    p: &theme::Palette,
    response: &egui::Response,
    layout: &PlaylistLayout,
) -> Option<Action> {
    drop_inside(ui, p, response, None, layout)
}

pub(super) fn folder_interaction(
    ui: &mut egui::Ui,
    p: &theme::Palette,
    response: &egui::Response,
    name: &str,
    layout: &PlaylistLayout,
    sidebar: bool,
) -> Option<Action> {
    remember_source(
        ui,
        response.rect,
        LibraryItem::Folder(name.to_owned()),
        !sidebar,
    );
    let mut action = None;
    if response.clicked() {
        action = Some(if sidebar {
            Action::Toggle(name.to_owned())
        } else {
            Action::Folder(Some(name.to_owned()))
        });
    }
    if response.drag_started()
        || (response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()))
    {
        egui::DragAndDrop::set_payload(ui.ctx(), FolderDrag(name.to_owned()));
    }
    let (on_edge, insert) = insertion_drop(
        ui,
        p,
        response.rect,
        LibraryItem::Folder(name.to_owned()),
        layout,
        sidebar,
        true,
    );
    if on_edge {
        action = insert.or(action);
    } else if let Some(drop) = drop_inside(ui, p, response, Some(name), layout) {
        action = Some(drop);
    }
    egui::Popup::context_menu(response)
        .frame(widgets::menu_frame(ui, p))
        .width(192.0)
        .show(|ui| {
            if sidebar
                && widgets::menu_item(ui, p, Icon::Library, "Open in Library", None).clicked()
            {
                action = Some(Action::Folder(Some(name.to_owned())));
                ui.close();
            }
            if widgets::menu_item(ui, p, Icon::ListMusic, "New playlist", None).clicked() {
                action = Some(Action::NewPlaylist(name.to_owned()));
                ui.close();
            }
            if widgets::menu_item(ui, p, Icon::Folder, "New subfolder", None).clicked() {
                action = Some(Action::NewFolder(Some(name.to_owned())));
                ui.close();
            }
            if let Some(parent) = layout.parent(name)
                && widgets::menu_item(ui, p, Icon::Library, "Move out of folder", None).clicked()
            {
                action = Some(Action::Nest(
                    name.to_owned(),
                    layout.parent(parent).map(str::to_owned),
                ));
                ui.close();
            }
            let order = layout.siblings(layout.parent(name));
            if let Some(i) = order.iter().position(|n| n.as_str() == name) {
                for (target, after, label) in [
                    (
                        i.checked_sub(1).and_then(|j| order.get(j)),
                        false,
                        "Move folder up",
                    ),
                    (order.get(i + 1), true, "Move folder down"),
                ] {
                    if let Some(target) = target
                        && widgets::menu_item(ui, p, Icon::ArrowUpDown, label, None).clicked()
                    {
                        action = Some(Action::Insert(
                            vec![LibraryItem::Folder(name.to_owned())],
                            LibraryItem::Folder(target.to_string()),
                            after,
                        ));
                        ui.close();
                    }
                }
            }
            if widgets::menu_item(ui, p, Icon::Shuffle, "Shuffle folder", None).clicked() {
                action = Some(Action::Shuffle(name.to_owned()));
                ui.close();
            }
            if widgets::menu_item(ui, p, Icon::Pencil, "Rename folder", None).clicked() {
                action = Some(Action::Rename(name.to_owned()));
                ui.close();
            }
            if widgets::menu_item(ui, p, Icon::Folder, "Ungroup playlists", None).clicked() {
                action = Some(Action::Remove(name.to_owned()));
                ui.close();
            }
        });
    action
}

pub(crate) fn grid(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;
    let view = &mut app.organizer;
    let layout = &app.settings.playlist_layout;
    let mut action = None;
    if let Some(folder) = &view.folder {
        ui.horizontal(|ui| {
            if widgets::icon_button(ui, &p, Icon::ChevronLeft, 18.0, false)
                .on_hover_text(
                    layout
                        .parent(folder)
                        .map(|p| layout.title(p))
                        .unwrap_or("All playlists"),
                )
                .clicked()
            {
                action = Some(Action::Folder(layout.parent(folder).map(str::to_owned)));
            }
            if widgets::icon_button(ui, &p, Icon::Shuffle, 17.0, false)
                .on_hover_text("Shuffle folder")
                .clicked()
            {
                action = Some(Action::Shuffle(folder.clone()));
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(layout.title(folder)).font(theme::bold(theme::SECTION_PT)),
                )
                .truncate(),
            );
        });
    }
    ui.horizontal_wrapped(|ui| {
        if !app.library.playlists.is_empty() {
            let w = (ui.available_width() - 220.0).clamp(110.0, 240.0);
            let input = widgets::text_input(ui, &p, &mut view.query, "Find a playlist", w);
            if input.changed() {
                view.finish_selection();
            }
            if !view.query.is_empty()
                && widgets::icon_button(ui, &p, Icon::X, 14.0, false)
                    .on_hover_text("Clear search")
                    .clicked()
            {
                view.query.clear();
                view.finish_selection();
            }
            if widgets::chip(
                ui,
                &p,
                if view.selecting { "Done" } else { "Select" },
                view.selecting,
            )
            .clicked()
            {
                if view.selecting {
                    view.finish_selection();
                } else {
                    view.selecting = true;
                }
            }
        }
        if widgets::outline_button(ui, &p, "New folder").clicked() {
            view.dialog = Some(Dialog::Name {
                parent: view.folder.clone(),
                previous: None,
                name: String::new(),
                ids: view.selected.clone(),
                focus: true,
            });
        }
    });
    let membership: HashMap<_, _> = layout
        .folders
        .iter()
        .flat_map(|(name, ids)| ids.iter().map(move |id| (id.as_str(), name.as_str())))
        .collect();
    let query = view.query.trim().to_lowercase();
    let mut visible: Vec<_> = app
        .library
        .playlists
        .iter()
        .filter(|pl| {
            if !query.is_empty() {
                pl.title.to_lowercase().contains(&query)
            } else {
                membership.get(pl.id.as_str()).copied() == view.folder.as_deref()
            }
        })
        .collect();
    let ranks = layout.ranks();
    visible.sort_by_cached_key(|pl| {
        ranks
            .get(&LibraryItem::Playlist(pl.id.clone()))
            .copied()
            .unwrap_or(usize::MAX)
    });
    let visible_ids: Vec<&str> = visible.iter().map(|pl| pl.id.as_str()).collect();
    if view.selecting && view.dialog.is_none() && !ui.ctx().egui_wants_keyboard_input() {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            view.finish_selection();
        } else if app.library.playlists.next.is_none()
            && !app.library.playlists.loading
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A))
        {
            view.selected
                .extend(visible_ids.iter().map(|id| id.to_string()));
        }
    }
    if view.selecting {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled_ui(
                    !app.library.playlists.loading && app.library.playlists.next.is_none(),
                    |ui| widgets::outline_button(ui, &p, "Select all"),
                )
                .inner
                .clicked()
            {
                view.selected
                    .extend(visible_ids.iter().map(|id| id.to_string()));
            }
            ui.label(
                egui::RichText::new(format!("{} selected", view.selected.len()))
                    .font(theme::regular(12.0))
                    .color(p.secondary),
            );
            if !view.selected.is_empty() {
                if widgets::pill_button(ui, &p, "Move to folder").clicked() {
                    action = Some(Action::Pick(view.selected.clone()));
                }
                if view
                    .selected
                    .iter()
                    .any(|id| membership.contains_key(id.as_str()))
                    && widgets::outline_button(ui, &p, "Move out of folder").clicked()
                {
                    action = Some(Action::MoveOut(view.selected.clone()));
                }
                let pin = !view.selected.is_subset(&layout.pins);
                if widgets::outline_button(ui, &p, if pin { "Pin to sidebar" } else { "Unpin" })
                    .clicked()
                {
                    action = Some(Action::Pin(view.selected.clone(), pin));
                }
            }
        });
    }
    ui.add_space(6.0);
    let mut last_visible = visible.is_empty();
    let order = layout.ordered_folders();
    let folders: Vec<_> = order
        .iter()
        .copied()
        .filter(|name| {
            if query.is_empty() {
                layout.parent(name) == view.folder.as_deref()
            } else {
                layout.title(name).to_lowercase().contains(&query)
            }
        })
        .collect();
    ui.horizontal_wrapped(|ui| {
        let w = super::grid_card_w(ui);
        let mut cards: Vec<_> = folders
            .iter()
            .map(|name| (LibraryItem::Folder((*name).clone()), None))
            .chain(
                visible
                    .iter()
                    .enumerate()
                    .map(|(i, pl)| (LibraryItem::Playlist(pl.id.clone()), Some(i))),
            )
            .collect();
        cards.sort_by_key(|(item, _)| ranks.get(item).copied().unwrap_or(usize::MAX));
        let card_count = cards.len();
        for (card_index, (item, index)) in cards.into_iter().enumerate() {
            if card_index + 1 == card_count {
                last_visible = super::card_is_visible(ui, w, widgets::ArtShape::Square);
            }
            if let LibraryItem::Folder(ref name) = item {
                let card = widgets::folder_card(
                    ui,
                    &p,
                    w,
                    layout.title(name),
                    &layout.description(name),
                    None,
                );
                if let Some(pick) = folder_interaction(ui, &p, &card.response, name, layout, false)
                {
                    action = Some(pick);
                }
                continue;
            }
            let pl = visible[index.expect("playlist card index")];
            let on_screen = super::card_is_visible(ui, w, widgets::ArtShape::Square);

            let art = on_screen
                .then(|| {
                    super::art_texture(
                        &mut app.images,
                        &app.backend,
                        ui.ctx(),
                        pl.thumbnail.as_ref(),
                        w - 16.0,
                    )
                })
                .flatten();
            let subtitle = if !query.is_empty() {
                membership
                    .get(pl.id.as_str())
                    .map(|id| layout.title(id))
                    .unwrap_or("")
            } else {
                ""
            };
            let selected = view.selected.contains(&pl.id);
            let card = widgets::interactive_art_card(
                ui,
                &p,
                w,
                art,
                &pl.title,
                subtitle,
                Some(match pl.source {
                    crate::model::PlaylistSource::YouTube => "YouTube Music",
                    crate::model::PlaylistSource::Spotify => "Spotify",
                    crate::model::PlaylistSource::Local if pl.id.starts_with("private:") => {
                        "Private playlist"
                    }
                    crate::model::PlaylistSource::Local => "Local file",
                }),
                widgets::ArtShape::Square,
                None,
                false,
                selected,
                true,
                !view.selecting,
            );
            if selected || layout.pins.contains(&pl.id) {
                let center = card.response.rect.min + egui::vec2(22.0, 22.0);
                ui.painter().circle_filled(center, 10.0, p.accent);
                let icon = if selected {
                    Icon::Check
                } else {
                    Icon::Bookmark
                };
                icon.image(p.on_accent, 13.0).paint_at(
                    ui,
                    egui::Rect::from_center_size(center, egui::vec2(13.0, 13.0)),
                );
            }
            if card.play {
                action = Some(Action::Play(pl.id.clone()));
            }
            if card.response.clicked() {
                let modifiers = ui.input(|i| i.modifiers);
                if view.selecting || modifiers.command || modifiers.shift {
                    view.select(&pl.id, &visible_ids, modifiers.shift);
                } else {
                    action = Some(Action::Open(pl.id.clone()));
                }
            }
            let targets = || {
                if selected {
                    view.selected.clone()
                } else {
                    BTreeSet::from([pl.id.clone()])
                }
            };
            remember_source(ui, card.response.rect, item.clone(), true);
            if card.response.drag_started()
                || (card.response.is_pointer_button_down_on()
                    && ui.input(|i| i.pointer.primary_pressed()))
            {
                egui::DragAndDrop::set_payload(
                    ui.ctx(),
                    Drag::from_grid(&pl.id, &view.selected, &visible_ids),
                );
            }
            if let (_, Some(insert)) =
                insertion_drop(ui, &p, card.response.rect, item, layout, false, false)
            {
                action = Some(insert);
            }
            egui::Popup::context_menu(&card.response)
                .frame(widgets::menu_frame(ui, &p))
                .width(192.0)
                .show(|ui| {
                    let pin = !layout.pins.contains(&pl.id)
                        || selected && !view.selected.is_subset(&layout.pins);
                    if widgets::menu_item(
                        ui,
                        &p,
                        Icon::Bookmark,
                        if pin {
                            "Pin to sidebar"
                        } else {
                            "Unpin from sidebar"
                        },
                        None,
                    )
                    .clicked()
                    {
                        action = Some(Action::Pin(targets(), pin));
                        ui.close();
                    }
                    if widgets::menu_item(ui, &p, Icon::Folder, "Move to folder", None).clicked() {
                        action = Some(Action::Pick(targets()));
                        ui.close();
                    }
                    if membership.contains_key(pl.id.as_str())
                        && widgets::menu_item(ui, &p, Icon::Library, "Move out of folder", None)
                            .clicked()
                    {
                        action = Some(Action::MoveOut(targets()));
                        ui.close();
                    }
                });
        }
    });
    // Mouse-down may be processed before the grid paints again. Keep the same
    // ordered selection for that fast path as for a normal drag-start event.
    view.selected_display_order = visible_ids
        .iter()
        .filter(|id| view.selected.contains(**id))
        .map(|id| (*id).to_owned())
        .collect();
    let paging = app.library.playlists.next.is_some() || app.library.playlists.loading;
    if visible.is_empty() && folders.is_empty() {
        let msg = if paging {
            "Loading playlists…"
        } else if !query.is_empty() {
            "No matching playlists"
        } else if view.folder.is_some() {
            "Drag playlists onto this folder in the sidebar."
        } else {
            "No playlists yet"
        };
        ui.label(
            egui::RichText::new(msg)
                .font(theme::regular(13.0))
                .color(p.secondary),
        );
    }
    if view.page_failed && widgets::outline_button(ui, &p, "Retry loading playlists").clicked() {
        view.page_failed = false;
    }
    if !view.page_failed
        && (last_visible || !query.is_empty() || view.folder.is_some() || view.selecting)
    {
        app.load_more_library(crate::api::LibrarySection::Playlists);
    }
    if let Some(action) = action {
        apply(app, action);
    }
}

pub(crate) fn dialog(app: &mut App, ctx: &egui::Context) {
    let Some(mut dialog) = app.organizer.dialog.take() else {
        return;
    };
    let p = app.palette;
    let mut next = None;
    let mut done = false;
    let modal = widgets::naming_modal("playlist-folder-dialog", &p).show(ctx, |ui| {
        ui.set_width((ctx.content_rect().width() - 70.0).clamp(200.0, 310.0));
        match &mut dialog {
            Dialog::Move(ids) => {
                ui.label(egui::RichText::new("Move to folder").font(theme::semibold(18.0)));
                ui.add_space(8.0);
                if widgets::menu_item(ui, &p, Icon::Plus, "New folder…", None).clicked() {
                    next = Some(Dialog::Name {
                        parent: app.organizer.folder.clone(),
                        previous: None,
                        name: String::new(),
                        ids: ids.clone(),
                        focus: true,
                    });
                }
                let mut dest = None;
                if widgets::menu_item(ui, &p, Icon::Library, "Library (no folder)", None).clicked()
                {
                    dest = Some(None);
                }
                egui::ScrollArea::vertical()
                    .max_height(ctx.content_rect().height() * 0.45)
                    .show(ui, |ui| {
                        let layout = &app.settings.playlist_layout;
                        for (name, _) in layout.tree(false) {
                            if widgets::menu_item(ui, &p, Icon::Folder, &layout.path(name), None)
                                .clicked()
                            {
                                dest = Some(Some(name.clone()));
                            }
                        }
                    });
                if let Some(folder) = dest {
                    apply(app, Action::Move(ids.clone(), folder));
                    done = true;
                }
            }
            Dialog::Name {
                parent,
                name,
                previous,
                ids,
                focus,
            } => {
                ui.label(
                    egui::RichText::new(if previous.is_some() {
                        "Rename folder"
                    } else {
                        "New folder"
                    })
                    .font(theme::semibold(18.0)),
                );
                ui.add_space(10.0);
                let entered =
                    widgets::name_field(ui, &p, name, "Folder name", std::mem::take(focus));
                let title = name.trim();
                let layout = &app.settings.playlist_layout;
                let taken = layout.siblings(parent.as_deref()).into_iter().any(|id| {
                    layout.title(id).eq_ignore_ascii_case(title) && Some(id) != previous.as_ref()
                });
                if taken {
                    ui.colored_label(p.warning, "A folder with this name already exists");
                }
                let too_long = title.chars().count() > 80;
                if too_long {
                    ui.colored_label(p.warning, "Use 80 characters or fewer");
                }
                let (submit, cancel) = widgets::name_actions(
                    ui,
                    &p,
                    if previous.is_some() { "Save" } else { "Create" },
                    !title.is_empty() && !taken && !too_long,
                    entered,
                );
                if submit {
                    let mut id = previous.clone().unwrap_or_default();
                    done = commit(app, |layout| {
                        if previous.is_some() {
                            layout.rename(&id, title);
                        } else {
                            id = layout.create_folder(title, parent.as_deref());
                            layout.move_to(ids, Some(&id));
                        }
                        layout.expanded_folders.insert(id.clone());
                    });
                    if done {
                        apply(app, Action::Folder(Some(id)));
                    }
                } else if cancel {
                    done = true;
                }
            }
        }
    });
    if let Some(next) = next {
        app.organizer.dialog = Some(next);
    } else if !done && !modal.should_close() {
        app.organizer.dialog = Some(dialog);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dragging_a_selection_uses_grid_order() {
        let selected = ["z".to_owned(), "a".to_owned()].into();
        assert_eq!(
            Drag::from_grid("a", &selected, &["z", "c", "a"]).0,
            ["z", "a"]
        );
        assert_eq!(Drag::from_grid("c", &selected, &["z", "c", "a"]).0, ["c"]);
    }

    #[test]
    fn a_batch_can_move_between_folders_without_losing_pins() {
        let mut layout = PlaylistLayout::default();
        let ids = BTreeSet::from(["private:a".into(), "VLabc".into()]);
        layout.pins = ids.clone();
        layout.move_to(&ids, Some("Old imports"));
        layout.move_to(&ids, Some("Night drives"));
        assert_eq!(layout.folders["Night drives"], ids);
        assert!(layout.folders["Old imports"].is_empty());
        layout.move_to(&ids, None);
        assert!(layout.folders["Night drives"].is_empty());
        assert_eq!(layout.pins, ids);
    }

    #[test]
    fn range_selection_uses_the_filtered_order() {
        let mut view = View::default();
        let visible = ["third", "first", "second"];
        view.select("third", &visible, false);
        view.select("second", &visible, true);
        assert_eq!(view.selected.len(), 3);
        view.select("first", &visible, false);
        assert!(!view.selected.contains("first"));
        view.finish_selection();
        assert!(view.selected.is_empty() && view.anchor.is_none() && !view.selecting);
    }
}
