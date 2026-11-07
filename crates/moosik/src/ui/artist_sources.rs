use std::sync::Arc;

use crate::app::{App, Route};
use crate::model::{Artist, ArtistLink, Entry, Provider, Shelf, Track};
use crate::source_groups::prioritize_entries;
use crate::{theme, widgets};

use super::source_rows;

#[derive(Default)]
struct Profile {
    artist: Option<Artist>,
    loading: bool,
    error: Option<String>,
}

struct ShelfSource {
    provider: Provider,
    id: String,
    name: String,
    shelves: Arc<Vec<Shelf>>,
}

struct DisplayShelf {
    sources: Vec<usize>,
    index: usize,
    shelf: Shelf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SharedSection {
    Discography(&'static str),
    RelatedArtists,
}

fn shared_section(shelf: &Shelf) -> Option<SharedSection> {
    if shelf.items.is_empty() {
        return None;
    }
    let title = shelf
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if matches!(title.as_str(), "fans might also like" | "fans also like")
        && shelf.items.iter().all(Entry::is_artist)
    {
        return Some(SharedSection::RelatedArtists);
    }
    if !shelf
        .items
        .iter()
        .all(|entry| matches!(entry, Entry::Album(_)))
    {
        return None;
    }
    let section = match title.as_str() {
        "albums" => "albums",
        "singles & eps" | "singles and eps" => "singles & eps",
        "singles" => "singles",
        "eps" => "eps",
        "compilations" => "compilations",
        "popular releases" => "popular releases",
        "discography" => "discography",
        _ => return None,
    };
    Some(SharedSection::Discography(section))
}

fn display_shelves(
    sources: &[ShelfSource],
    combined: bool,
    preferred: Option<Provider>,
) -> Vec<DisplayShelf> {
    let mut shelves: Vec<DisplayShelf> = Vec::new();
    for (source_index, source) in sources.iter().enumerate() {
        for (index, original) in source.shelves.iter().enumerate() {
            if original.items.is_empty() {
                continue;
            }
            let section = combined.then(|| shared_section(original)).flatten();
            let mut shelf = original.clone();
            if let Some(SharedSection::Discography(section)) = section {
                let kind = match section {
                    "albums" => Some("Album"),
                    "singles" => Some("Single"),
                    "eps" => Some("EP"),
                    "compilations" => Some("Compilation"),
                    _ => None,
                };
                for entry in &mut shelf.items {
                    let Entry::Album(album) = entry else {
                        continue;
                    };
                    if album.artist.trim().is_empty()
                        && album.artists.is_empty()
                        && !source.name.trim().is_empty()
                    {
                        album.artist = source.name.clone();
                        album.artists.push(ArtistLink {
                            name: source.name.clone(),
                            id: Some(source.id.clone()),
                        });
                    }
                    if album.kind.is_none() {
                        album.kind = kind.map(str::to_owned);
                    }
                }
            }
            if let Some(section) = section
                && let Some(existing) = shelves.iter_mut().find(|existing| {
                    !existing.sources.contains(&source_index)
                        && shared_section(&existing.shelf) == Some(section)
                })
            {
                existing.sources.push(source_index);
                existing.shelf.items.extend(shelf.items);
                if section == SharedSection::RelatedArtists {
                    existing.shelf.title = "Fans also like".to_owned();
                }
            } else {
                shelves.push(DisplayShelf {
                    sources: vec![source_index],
                    index,
                    shelf,
                });
            }
        }
    }
    for shelf in &mut shelves {
        if shelf.sources.len() > 1 {
            prioritize_entries(&mut shelf.shelf.items, preferred);
        }
    }
    shelves
}

#[derive(Default)]
pub(crate) struct State {
    pair: Option<(String, String)>,
    youtube: Profile,
    spotify: Profile,
    entries_preferred: Option<Provider>,
    pub entries: Vec<Entry>,
    pub revision: u64,
    pub rows: source_rows::State,
}

impl State {
    pub fn begin(&mut self, youtube: &str, spotify: &str) -> bool {
        if self.ids() == Some((youtube, spotify)) {
            return false;
        }
        self.clear();
        self.pair = Some((youtube.to_owned(), spotify.to_owned()));
        self.youtube.loading = true;
        self.spotify.loading = true;
        true
    }

    pub fn ids(&self) -> Option<(&str, &str)> {
        self.pair
            .as_ref()
            .map(|(youtube, spotify)| (youtube.as_str(), spotify.as_str()))
    }

    pub fn youtube_id(&self) -> Option<&str> {
        self.ids().map(|(youtube, _)| youtube)
    }

    pub fn spotify_id(&self) -> Option<&str> {
        self.ids().map(|(_, spotify)| spotify)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids()
            .is_some_and(|(youtube, spotify)| id == youtube || id == spotify)
    }

    pub fn loaded(&mut self, id: &str, artist: Artist) {
        let Some(profile) = self.profile_mut(id) else {
            return;
        };
        profile.artist = Some(artist);
        profile.loading = false;
        profile.error = None;
        self.rebuild_entries();
    }

    pub fn failed(&mut self, id: &str, error: String) {
        let Some(profile) = self.profile_mut(id) else {
            return;
        };
        profile.loading = false;
        profile.error = Some(error);
    }

    pub fn clear(&mut self) {
        self.pair = None;
        self.youtube = Profile::default();
        self.spotify = Profile::default();
        self.entries_preferred = None;
        self.entries.clear();
        self.revision = self.revision.wrapping_add(1);
        self.rows = source_rows::State::default();
    }

    pub fn retry(&mut self) -> Vec<String> {
        let Some((youtube, spotify)) = self.pair.clone() else {
            return Vec::new();
        };
        let mut pending = Vec::new();
        for (id, profile) in [(youtube, &mut self.youtube), (spotify, &mut self.spotify)] {
            if !profile.loading && (profile.error.is_some() || profile.artist.is_none()) {
                profile.loading = true;
                profile.error = None;
                pending.push(id);
            }
        }
        pending
    }

    pub fn profile(&self, provider: Provider) -> Option<&Artist> {
        match provider {
            Provider::YouTubeMusic => self.youtube.artist.as_ref(),
            Provider::Spotify => self.spotify.artist.as_ref(),
            Provider::Local => None,
        }
    }

    pub fn update_library_tracks(
        &mut self,
        video_id: &str,
        mut update: impl FnMut(&mut Track) -> bool,
    ) {
        let mut top_songs_changed = false;
        for artist in [&mut self.youtube.artist, &mut self.spotify.artist]
            .into_iter()
            .flatten()
        {
            for track in &mut artist.top_songs {
                if track.video_id == video_id {
                    top_songs_changed |= update(track);
                }
            }
            if artist
                .shelves
                .iter()
                .flat_map(|shelf| &shelf.items)
                .filter_map(Entry::track)
                .any(|track| track.video_id == video_id)
            {
                for track in Arc::make_mut(&mut artist.shelves)
                    .iter_mut()
                    .flat_map(|shelf| &mut shelf.items)
                    .filter_map(Entry::track_mut)
                {
                    if track.video_id == video_id {
                        update(track);
                    }
                }
            }
        }
        if top_songs_changed {
            self.rebuild_entries();
        }
    }

    fn preferred_profile(&self, provider: Provider) -> Option<&Artist> {
        self.profile(provider)
            .or(self.youtube.artist.as_ref())
            .or(self.spotify.artist.as_ref())
    }

    fn profile_mut(&mut self, id: &str) -> Option<&mut Profile> {
        if self.youtube_id() == Some(id) {
            Some(&mut self.youtube)
        } else if self.spotify_id() == Some(id) {
            Some(&mut self.spotify)
        } else {
            None
        }
    }

    fn set_entry_preference(&mut self, preferred: Option<Provider>) {
        if self.entries_preferred != preferred {
            self.entries_preferred = preferred;
            self.rebuild_entries();
        }
    }

    fn rebuild_entries(&mut self) {
        let mut entries = self
            .youtube
            .artist
            .iter()
            .chain(self.spotify.artist.iter())
            .flat_map(|artist| artist.top_songs.iter().cloned().map(Entry::Song))
            .collect();
        prioritize_entries(&mut entries, self.entries_preferred);
        if self.entries != entries {
            self.entries = entries;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    fn errors(&self) -> Vec<String> {
        [
            (Provider::YouTubeMusic, self.youtube.error.as_deref()),
            (Provider::Spotify, self.spotify.error.as_deref()),
        ]
        .into_iter()
        .filter_map(|(provider, error)| error.map(|error| format!("{}: {error}", provider.name())))
        .collect()
    }
}

pub(crate) fn page(app: &mut App, ui: &mut egui::Ui) {
    let combined = app.can_group_sources();
    let preferred = combined.then_some(app.settings.preferred_provider);
    app.artist_sources.set_entry_preference(preferred);
    let header = app
        .artist_sources
        .preferred_profile(app.settings.preferred_provider)
        .map(|artist| (artist.name.clone(), artist.thumbnail.clone()));
    let errors = app.artist_sources.errors();
    let loading = app.artist_sources.youtube.loading || app.artist_sources.spotify.loading;
    let Some((name, artwork)) = header else {
        if loading {
            super::skeleton_page_header(
                ui,
                &app.palette,
                super::HeaderSkeleton {
                    kind: super::HeaderKind::None,
                    title: None,
                    description: false,
                    credit_line: true,
                    credit_air: 4.0,
                    controls: super::HeaderControls::PlayOnly,
                },
            );
            ui.add_space(10.0);
            widgets::skeleton_track_rows(ui, &app.palette, 8, false);
        } else if !errors.is_empty() {
            super::failed(app, ui, errors.join("\n"), retry);
        }
        return;
    };
    let sources: Vec<_> = [Provider::YouTubeMusic, Provider::Spotify]
        .into_iter()
        .filter_map(|provider| {
            let id = match provider {
                Provider::YouTubeMusic => app.artist_sources.youtube_id(),
                Provider::Spotify => app.artist_sources.spotify_id(),
                Provider::Local => None,
            }?;
            let artist = app.artist_sources.profile(provider);
            Some(ShelfSource {
                provider,
                id: id.to_owned(),
                name: artist.map(|artist| artist.name.clone()).unwrap_or_default(),
                shelves: artist
                    .map(|artist| Arc::clone(&artist.shelves))
                    .unwrap_or_default(),
            })
        })
        .collect();
    let shelves = display_shelves(&sources, combined, preferred);
    let marks = [Provider::YouTubeMusic, Provider::Spotify]
        .into_iter()
        .filter_map(|provider| {
            app.artist_sources
                .profile(provider)
                .and_then(super::artist::VerificationMark::for_artist)
        })
        .collect::<Vec<_>>();

    super::page_scroll(app, "artist-sources").show(ui, |ui| {
        ui.add_space(theme::HEADER_TOP_PAD);
        let width = ui.available_width();
        let gutter = theme::PAGE_INSET_X;
        let art_size =
            super::HEADER_ART_PT.min((width - gutter - 224.0).max(super::HEADER_ART_FLOOR_PT));
        let text_width = (width - art_size - gutter).max(0.0);
        let art = super::art_texture(
            &mut app.images,
            &app.backend,
            ui.ctx(),
            artwork.as_ref(),
            super::HEADER_ART_PT,
        );
        let mut selected = None;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gutter;
            let (art_rect, _) =
                ui.allocate_exact_size(egui::Vec2::splat(art_size), egui::Sense::hover());
            widgets::paint_art(ui, &app.palette, art_rect, art, widgets::ArtShape::Circle);
            ui.vertical(|ui| {
                ui.set_width(text_width);
                let title_width =
                    (text_width - super::artist::verification_width(marks.len())).max(0.0);
                let title_size = widgets::fit_size(
                    ui,
                    &name,
                    title_width,
                    super::DETAIL_TITLE_MAX,
                    super::DETAIL_TITLE_MIN,
                    theme::bold,
                );
                let title_font = theme::bold(title_size);
                let title = widgets::truncate_to(ui, &name, title_font.clone(), title_width);
                let (title_rect, _) = ui.allocate_exact_size(
                    egui::vec2(text_width, title_size + 4.0),
                    egui::Sense::hover(),
                );
                let title_pos = title_rect.left_center();
                let title_advance =
                    crate::emoji::paint_line(ui, title_pos, &title, title_font, app.palette.text);
                super::artist::paint_verification_marks(
                    ui,
                    app.palette.panel,
                    title_pos + egui::vec2(title_advance, 0.0),
                    marks.iter().copied(),
                );
                ui.add_space(6.0);
                let inline = text_width >= 540.0;
                for source in &sources {
                    if profile_row(
                        ui,
                        &app.palette,
                        source.provider,
                        app.artist_sources.profile(source.provider),
                        inline,
                    ) {
                        selected = Some(source.id.clone());
                    }
                }
            });
        });
        ui.add_space(8.0);
        if let Some(id) = selected {
            app.navigate(Route::Artist(id));
            return;
        }
        for error in &errors {
            ui.label(
                egui::RichText::new(error)
                    .font(theme::regular(13.0))
                    .color(app.palette.danger),
            );
        }
        if !errors.is_empty() && widgets::outline_button(ui, &app.palette, "Try again").clicked() {
            retry(app);
        }
        for (provider, pending) in [
            (Provider::YouTubeMusic, app.artist_sources.youtube.loading),
            (Provider::Spotify, app.artist_sources.spotify.loading),
        ] {
            if pending {
                ui.label(
                    egui::RichText::new(format!("Loading {}…", provider.name()))
                        .font(theme::regular(13.0))
                        .color(app.palette.dim),
                );
            }
        }
        if !app.artist_sources.entries.is_empty() {
            super::section(ui, &app.palette, "Popular");
            let mut rows = std::mem::take(&mut app.artist_sources.rows);
            rows.set_entries(
                &app.artist_sources.entries,
                app.artist_sources.revision,
                combined,
            );
            source_rows::draw(
                app,
                ui,
                &mut rows,
                "artist-sources",
                source_rows::Playback::List,
            );
            app.artist_sources.rows = rows;
        }
        for shelf in &shelves {
            let source = &sources[shelf.sources[0]];
            let combined = shelf.sources.len() > 1;
            ui.push_id(("artist-source-shelf", &source.id, shelf.index), |ui| {
                ui.add_space(20.0);
                if !combined {
                    ui.label(
                        egui::RichText::new(source.provider.name())
                            .font(theme::regular(12.0))
                            .color(app.palette.dim),
                    );
                }
                super::shelf_carousel(
                    app,
                    ui,
                    &source.id,
                    &shelf.shelf,
                    (!combined).then(|| Route::Artist(source.id.clone())),
                    super::MoreButton::Pill("Open profile"),
                );
            });
        }
    });
}

fn profile_row(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    provider: Provider,
    artist: Option<&Artist>,
    inline: bool,
) -> bool {
    const LINK_WIDTH: f32 = 125.0;
    const LINK_GAP: f32 = 12.0;
    const TEXT_INSET: f32 = 6.0;
    let details = artist
        .into_iter()
        .flat_map(Artist::audience_stats)
        .collect::<Vec<_>>()
        .join(widgets::FIELD_SEP);
    let detail_label = || {
        egui::Label::new(
            egui::RichText::new(&details)
                .font(theme::regular(13.0))
                .color(palette.secondary),
        )
        .wrap()
    };
    let profile_link = |ui: &mut egui::Ui| {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(LINK_WIDTH, 28.0), egui::Sense::click());
        let label = format!("Open {} profile", provider.name());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Link, ui.is_enabled(), &label)
        });
        let radius = egui::CornerRadius::same(theme::RADIUS_SMALL);
        if response.hovered() || response.has_focus() {
            ui.painter()
                .rect_filled(rect, radius, palette.surface_hover);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect,
                radius,
                egui::Stroke::new(1.0, palette.text),
                egui::StrokeKind::Inside,
            );
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let center = |x| egui::pos2(x, rect.center().y);
        ui.painter().text(
            center(rect.left() + TEXT_INSET),
            egui::Align2::LEFT_CENTER,
            provider.name(),
            theme::semibold(13.0),
            palette.text,
        );
        crate::icons::Icon::ChevronRight
            .image(palette.dim, 13.0)
            .paint_at(
                ui,
                egui::Rect::from_center_size(center(rect.right() - 11.0), egui::Vec2::splat(13.0)),
            );
        response.on_hover_text(label).clicked()
    };
    if inline {
        return ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = LINK_GAP;
                let clicked = profile_link(ui);
                if !details.is_empty() {
                    ui.add(detail_label());
                }
                clicked
            })
            .inner;
    }
    let clicked = profile_link(ui);
    if !details.is_empty() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(TEXT_INSET);
            ui.add(detail_label());
        });
        ui.add_space(4.0);
    }
    clicked
}

fn retry(app: &mut App) {
    for id in app.artist_sources.retry() {
        app.backend.fetch_artist(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, title: &str, artist: &str) -> Entry {
        Entry::Album(crate::model::AlbumRef {
            id: id.to_owned(),
            title: title.to_owned(),
            artist: artist.to_owned(),
            artists: Vec::new(),
            explicit: None,
            year: Some("2026".to_owned()),
            kind: Some("Album".to_owned()),
            thumbnail: None,
        })
    }

    fn artist(id: &str, name: &str) -> Entry {
        Entry::Artist(crate::model::ArtistRef {
            id: id.to_owned(),
            name: name.to_owned(),
            subscribers: None,
            thumbnail: None,
            actions: crate::model::ArtistActions::default(),
        })
    }

    fn shelf(title: &str, items: Vec<Entry>) -> Shelf {
        Shelf {
            title: title.to_owned(),
            strapline: None,
            items,
            more_params: Some("more".to_owned()),
            more_browse_id: Some("browse".to_owned()),
            expanded: false,
        }
    }

    fn source(provider: Provider, shelves: Vec<Shelf>) -> ShelfSource {
        ShelfSource {
            provider,
            id: match provider {
                Provider::YouTubeMusic => "UCArtist".to_owned(),
                Provider::Spotify => "spotify:artist:0000000000000000000000".to_owned(),
                Provider::Local => "local".to_owned(),
            },
            name: "Artist".to_owned(),
            shelves: Arc::new(shelves),
        }
    }

    #[test]
    fn library_changes_refresh_cached_artist_rows() {
        let id = "spotify:track:0000000000000000000000";
        let spotify = "spotify:artist:0000000000000000000000";
        let track = crate::listening::Metadata {
            id: id.to_owned(),
            title: "Song".to_owned(),
            ..Default::default()
        }
        .to_track();
        let mut state = State::default();
        state.begin("UCArtist", spotify);
        state.loaded(
            spotify,
            Artist {
                id: spotify.to_owned(),
                name: "Artist".to_owned(),
                verified: false,
                official_artist_channel: false,
                subscribers: None,
                subscribers_long: None,
                followers: None,
                monthly_listeners: None,
                views: None,
                actions: Default::default(),
                description: None,
                thumbnail: None,
                hero: None,
                top_songs: vec![track.clone()],
                top_songs_title: None,
                top_songs_playlist: None,
                shelves: Arc::new(vec![shelf("Songs", vec![Entry::Song(track)])]),
            },
        );

        let mut revision = state.revision;
        state.rows.set_entries(&state.entries, revision, true);
        for (in_library, changed) in [(true, true), (true, false), (false, true)] {
            state.update_library_tracks(id, |track| {
                std::mem::replace(&mut track.in_library, in_library) != in_library
            });
            assert_eq!(state.revision != revision, changed);
            revision = state.revision;
            state.rows.set_entries(&state.entries, revision, true);
            let (tracks, index) = state
                .rows
                .playback_queue(0, Provider::Spotify, None)
                .unwrap();
            assert_eq!(tracks[index].in_library, in_library);
            let artist = state.profile(Provider::Spotify).unwrap();
            assert_eq!(artist.top_songs[0].in_library, in_library);
            assert_eq!(
                artist.shelves[0].items[0].track().unwrap().in_library,
                in_library
            );
        }
    }

    #[test]
    fn matching_discography_shelves_keep_raw_variants_and_source_order() {
        let sources = [
            source(
                Provider::YouTubeMusic,
                vec![shelf(
                    "Albums",
                    vec![album("yt", "Record", ""), album("unique", "Another", "")],
                )],
            ),
            source(
                Provider::Spotify,
                vec![shelf(
                    "Albums",
                    vec![
                        album("spotify:album:0000000000000000000000", "Record", "Artist"),
                        album(
                            "spotify:album:1111111111111111111111",
                            "Record (Deluxe)",
                            "Artist",
                        ),
                    ],
                )],
            ),
        ];
        let shelves = display_shelves(&sources, true, None);
        assert_eq!(shelves.len(), 1);
        assert_eq!(shelves[0].sources, [0, 1]);
        assert_eq!(
            shelves[0]
                .shelf
                .items
                .iter()
                .map(Entry::title)
                .collect::<Vec<_>>(),
            ["Record", "Record", "Another", "Record (Deluxe)"]
        );
        let groups = crate::source_groups::group_entries(&shelves[0].shelf.items, true);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].variants.len(), 2);
        let Entry::Album(album) = &shelves[0].shelf.items[0] else {
            panic!()
        };
        assert_eq!(
            album.artists,
            [ArtistLink {
                name: "Artist".to_owned(),
                id: Some("UCArtist".to_owned()),
            }]
        );
        let Entry::Album(original) = &sources[0].shelves[0].items[0] else {
            panic!()
        };
        assert!(original.artist.is_empty());
        assert!(original.artists.is_empty());
    }

    #[test]
    fn related_artist_aliases_join_one_shelf_with_preferred_source_order() {
        let youtube = artist("UCrelated", "Related");
        let spotify = artist("spotify:artist:1111111111111111111111", "Related");
        let youtube_only = artist("UConly", "YouTube only");
        let spotify_only = artist("spotify:artist:2222222222222222222222", "Spotify only");
        let mixed = shelf(
            "Fans also like",
            vec![spotify.clone(), album("yt", "Record", "Artist")],
        );
        let sources = [
            source(
                Provider::YouTubeMusic,
                vec![shelf(
                    "Fans might also like",
                    vec![youtube.clone(), youtube_only.clone()],
                )],
            ),
            source(
                Provider::Spotify,
                vec![
                    shelf(
                        "Fans also like",
                        vec![spotify.clone(), spotify_only.clone()],
                    ),
                    mixed.clone(),
                ],
            ),
        ];
        let shelves = display_shelves(&sources, true, Some(Provider::Spotify));
        assert_eq!(shelves.len(), 2);
        assert_eq!(shelves[0].sources, [0, 1]);
        assert_eq!(shelves[0].shelf.title, "Fans also like");
        assert_eq!(
            shelves[0].shelf.items,
            [spotify, spotify_only, youtube, youtube_only]
        );
        let groups = crate::source_groups::group_entries(&shelves[0].shelf.items, true);
        assert_eq!(groups.len(), 3);
        assert_eq!(
            groups[0].providers(),
            [Provider::Spotify, Provider::YouTubeMusic]
        );
        assert_eq!(shelves[1].sources, [1]);
        assert_eq!(shelves[1].shelf, mixed);
    }

    #[test]
    fn other_shelves_keep_their_entries_metadata_and_native_profile() {
        let mut track = crate::listening::Metadata {
            id: "video".to_owned(),
            title: "Video".to_owned(),
            ..Default::default()
        }
        .to_track();
        track.wide = true;
        let videos = shelf("Videos", vec![Entry::Video(track)]);
        let featured = shelf("Featured on", vec![album("featured", "Compilation", "")]);
        let mut credited = album("credited", "Collaboration", "Other Artist");
        if let Entry::Album(album) = &mut credited {
            album.artists = vec![ArtistLink::plain("Other Artist")];
            album.year = None;
            album.kind = Some("EP".to_owned());
        }
        let sources = [
            source(
                Provider::YouTubeMusic,
                vec![
                    shelf("Albums", vec![credited.clone()]),
                    videos.clone(),
                    featured.clone(),
                ],
            ),
            source(
                Provider::Spotify,
                vec![
                    shelf(
                        "Albums",
                        vec![album(
                            "spotify:album:0000000000000000000000",
                            "Record",
                            "Artist",
                        )],
                    ),
                    shelf("Appears on", vec![album("appears", "Guest", "")]),
                ],
            ),
        ];
        let shelves = display_shelves(&sources, true, None);
        assert_eq!(shelves.len(), 4);
        assert_eq!(shelves[0].shelf.items[0], credited);
        assert_eq!(shelves[1].sources, [0]);
        assert_eq!(shelves[1].index, 1);
        assert_eq!(shelves[1].shelf, videos);
        assert_eq!(shelves[2].shelf, featured);
        assert_eq!(shelves[3].sources, [1]);
        assert_eq!(shelves[3].shelf, sources[1].shelves[1]);
    }

    #[test]
    fn disabled_grouping_keeps_provider_shelves_separate_and_unmodified() {
        let sources = [
            source(
                Provider::YouTubeMusic,
                vec![shelf("Albums", vec![album("yt", "Record", "")])],
            ),
            source(
                Provider::Spotify,
                vec![shelf("Albums", vec![album("sp", "Record", "Artist")])],
            ),
        ];
        let shelves = display_shelves(&sources, false, Some(Provider::Spotify));
        assert_eq!(shelves.len(), 2);
        for (index, shelf) in shelves.iter().enumerate() {
            assert_eq!(shelf.sources, [index]);
            assert_eq!(shelf.shelf, sources[index].shelves[0]);
        }
    }

    #[test]
    fn albums_shelf_supplies_missing_kind_for_shared_cards_and_keeps_editions() {
        let mut youtube = album("MPREb_6c5YkS1Y0E", "Record", "");
        let mut deluxe = album("MPREb_6c5YkS1Y0F", "Record (Deluxe)", "");
        for entry in [&mut youtube, &mut deluxe] {
            let Entry::Album(album) = entry else { panic!() };
            album.kind = None;
        }
        let sources = [
            source(
                Provider::YouTubeMusic,
                vec![shelf("Albums", vec![youtube, deluxe])],
            ),
            source(
                Provider::Spotify,
                vec![shelf(
                    "Albums",
                    vec![album(
                        "spotify:album:0000000000000000000000",
                        "Record",
                        "Artist",
                    )],
                )],
            ),
        ];
        let shelves = display_shelves(&sources, true, None);
        let groups = crate::source_groups::group_entries(&shelves[0].shelf.items, true);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[0].providers(),
            [Provider::YouTubeMusic, Provider::Spotify]
        );
        assert_eq!(groups[1].variants[0].title(), "Record (Deluxe)");
        let Entry::Album(original) = &sources[0].shelves[0].items[0] else {
            panic!()
        };
        assert!(original.kind.is_none());
    }

    #[test]
    fn mixed_discography_shelves_keep_missing_kind_unknown() {
        for title in ["Singles & EPs", "Popular releases", "Discography"] {
            let mut youtube = album("MPREb_6c5YkS1Y0E", "Record", "");
            let Entry::Album(youtube_album) = &mut youtube else {
                panic!()
            };
            youtube_album.kind = None;
            let sources = [
                source(Provider::YouTubeMusic, vec![shelf(title, vec![youtube])]),
                source(
                    Provider::Spotify,
                    vec![shelf(
                        title,
                        vec![album(
                            "spotify:album:0000000000000000000000",
                            "Record",
                            "Artist",
                        )],
                    )],
                ),
            ];
            let shelves = display_shelves(&sources, true, None);
            let groups = crate::source_groups::group_entries(&shelves[0].shelf.items, true);
            assert_eq!(groups.len(), 2, "{title}");
            let Entry::Album(album) = &shelves[0].shelf.items[0] else {
                panic!()
            };
            assert!(album.kind.is_none(), "{title}");
        }
    }
}
