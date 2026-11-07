use std::sync::Arc;

use crate::app::{App, Route};
use crate::model::{Artist, ArtistLink, Entry, Provider, Shelf};
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

fn discography_section(shelf: &Shelf) -> Option<&'static str> {
    if shelf.items.is_empty()
        || !shelf
            .items
            .iter()
            .all(|entry| matches!(entry, Entry::Album(_)))
    {
        return None;
    }
    match shelf
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .as_str()
    {
        "albums" => Some("albums"),
        "singles & eps" | "singles and eps" => Some("singles & eps"),
        "singles" => Some("singles"),
        "eps" => Some("eps"),
        "compilations" => Some("compilations"),
        "popular releases" => Some("popular releases"),
        "discography" => Some("discography"),
        _ => None,
    }
}

fn display_shelves(sources: &[ShelfSource], combined: bool) -> Vec<DisplayShelf> {
    let mut shelves: Vec<DisplayShelf> = Vec::new();
    for (source_index, source) in sources.iter().enumerate() {
        for (index, original) in source.shelves.iter().enumerate() {
            if original.items.is_empty() {
                continue;
            }
            let section = combined.then(|| discography_section(original)).flatten();
            let mut shelf = original.clone();
            if let Some(section) = section {
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
                        && discography_section(&existing.shelf) == Some(section)
                })
            {
                existing.sources.push(source_index);
                existing.shelf.items.extend(shelf.items);
            } else {
                shelves.push(DisplayShelf {
                    sources: vec![source_index],
                    index,
                    shelf,
                });
            }
        }
    }
    shelves
}

#[derive(Default)]
pub(crate) struct State {
    pair: Option<(String, String)>,
    youtube: Profile,
    spotify: Profile,
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

    fn rebuild_entries(&mut self) {
        let youtube = self
            .youtube
            .artist
            .as_ref()
            .map(|artist| artist.top_songs.as_slice())
            .unwrap_or_default();
        let spotify = self
            .spotify
            .artist
            .as_ref()
            .map(|artist| artist.top_songs.as_slice())
            .unwrap_or_default();
        let mut entries = Vec::with_capacity(youtube.len() + spotify.len());
        for index in 0..youtube.len().max(spotify.len()) {
            for track in [youtube.get(index), spotify.get(index)]
                .into_iter()
                .flatten()
            {
                entries.push(Entry::Song(track.clone()));
            }
        }
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
    let shelves = display_shelves(&sources, app.can_group_sources());

    super::page_scroll(app, "artist-sources").show(ui, |ui| {
        ui.add_space(theme::HEADER_TOP_PAD);
        let width = ui.available_width();
        let gutter = theme::PAGE_INSET_X;
        let art_size =
            super::HEADER_ART_PT.min((width - gutter - 224.0).max(super::HEADER_ART_FLOOR_PT));
        let text_width = (width - art_size - gutter).max(0.0);
        let title_size = widgets::fit_size(
            ui,
            &name,
            text_width,
            super::DETAIL_TITLE_MAX,
            super::DETAIL_TITLE_MIN,
            theme::bold,
        );
        let title_font = theme::bold(title_size);
        let title = widgets::truncate_to(ui, &name, title_font.clone(), text_width);
        let title_bottom = title_size * 0.5 + widgets::ink_drop(ui, &title, &title_font);
        let controls_height = if text_width < 224.0 { 72.0 } else { 32.0 };
        let text_height = title_bottom + 16.0 + controls_height;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(width, art_size.max(text_height)),
            egui::Sense::hover(),
        );
        let art_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - art_size * 0.5),
            egui::Vec2::splat(art_size),
        );
        let art = super::art_texture(
            &mut app.images,
            &app.backend,
            ui.ctx(),
            artwork.as_ref(),
            super::HEADER_ART_PT,
        );
        widgets::paint_art(ui, &app.palette, art_rect, art, widgets::ArtShape::Circle);
        let text_x = art_rect.right() + gutter;
        let text_top = rect.center().y - text_height * 0.5;
        crate::emoji::paint_line(
            ui,
            egui::pos2(text_x, text_top + title_size * 0.5),
            &title,
            title_font,
            app.palette.text,
        );
        let mut controls = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("artist-sources-profiles")
                .max_rect(egui::Rect::from_min_size(
                    egui::pos2(text_x, text_top + title_bottom + 16.0),
                    egui::vec2(text_width, controls_height),
                ))
                .layout(if text_width < 224.0 {
                    egui::Layout::top_down(egui::Align::Min)
                } else {
                    egui::Layout::left_to_right(egui::Align::Center)
                }),
        );
        controls.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        let mut selected = None;
        for source in &sources {
            if widgets::outline_button(&mut controls, &app.palette, source.provider.name())
                .clicked()
            {
                selected = Some(source.id.clone());
            }
        }
        ui.advance_cursor_after_rect(rect.union(controls.min_rect()));
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
            let enabled = app.can_group_sources();
            let mut rows = std::mem::take(&mut app.artist_sources.rows);
            rows.set_entries(
                &app.artist_sources.entries,
                app.artist_sources.revision,
                enabled,
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
        let shelves = display_shelves(&sources, true);
        assert_eq!(shelves.len(), 1);
        assert_eq!(shelves[0].sources, [0, 1]);
        assert_eq!(
            shelves[0]
                .shelf
                .items
                .iter()
                .map(Entry::title)
                .collect::<Vec<_>>(),
            ["Record", "Another", "Record", "Record (Deluxe)"]
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
        let shelves = display_shelves(&sources, true);
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
        let shelves = display_shelves(&sources, false);
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
        let shelves = display_shelves(&sources, true);
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
            let shelves = display_shelves(&sources, true);
            let groups = crate::source_groups::group_entries(&shelves[0].shelf.items, true);
            assert_eq!(groups.len(), 2, "{title}");
            let Entry::Album(album) = &shelves[0].shelf.items[0] else {
                panic!()
            };
            assert!(album.kind.is_none(), "{title}");
        }
    }
}
