use std::collections::{HashMap, HashSet};

use crate::app::{App, PlaylistSort, Route};
use crate::model::{Entry, LikeStatus, Paged, Provider, Track};
use crate::{theme, widgets};

use super::source_rows;

pub(crate) const SOURCES: [Provider; 2] = [Provider::YouTubeMusic, Provider::Spotify];
pub(crate) const TITLE: &str = "Liked Music";

#[derive(Default)]
struct Source {
    songs: Paged<Track>,
    loaded: bool,
    error: Option<String>,
    cursors: HashSet<String>,
    ids: HashSet<String>,
    generation: u64,
}

impl Source {
    fn complete(&self) -> bool {
        self.loaded && self.songs.next.is_none() && self.error.is_none()
    }
}

#[derive(Default)]
pub(crate) struct State {
    sources: [Source; 2],
    removed_youtube: HashMap<String, Track>,
    removed_spotify: HashSet<String>,
    pub(crate) spotify_pending: HashSet<String>,
    filter: Option<Provider>,
    sort: PlaylistSort,
    revision: u64,
    projection: Option<(u64, [bool; 2], PlaylistSort, Provider)>,
    rows: source_rows::State,
}

impl State {
    fn source(&self, provider: Provider) -> &Source {
        &self.sources[usize::from(provider == Provider::Spotify)]
    }

    fn source_mut(&mut self, provider: Provider) -> &mut Source {
        &mut self.sources[usize::from(provider == Provider::Spotify)]
    }

    pub(crate) fn generation(&self, provider: Provider) -> u64 {
        self.source(provider).generation
    }

    pub(crate) fn request(&mut self, provider: Provider, more: bool) -> Option<Option<String>> {
        let source = self.source_mut(provider);
        if source.songs.loading || source.error.is_some() {
            return None;
        }
        let token = if source.loaded {
            if !more {
                return None;
            }
            Some(source.songs.next.clone()?)
        } else {
            None
        };
        source.songs.loading = true;
        Some(token)
    }

    pub(crate) fn loaded(&mut self, provider: Provider, result: Result<Paged<Track>, String>) {
        let removed_youtube = &self.removed_youtube;
        let removed_spotify = &self.removed_spotify;
        let source = &mut self.sources[usize::from(provider == Provider::Spotify)];
        source.songs.loading = false;
        let page = match result {
            Ok(page) => page,
            Err(error) => {
                source.error = Some(error);
                return;
            }
        };
        if let Some(next) = &page.next
            && !source.cursors.insert(next.clone())
        {
            source.error = Some("The next page repeated an earlier page.".to_owned());
            return;
        }
        let songs = page
            .items
            .into_iter()
            .filter(|track| {
                !match provider {
                    Provider::YouTubeMusic => removed_youtube.contains_key(&track.video_id),
                    Provider::Spotify => removed_spotify.contains(&track.video_id),
                    Provider::Local => false,
                } && source.ids.insert(track.video_id.clone())
            })
            .map(|mut track| {
                track.like = LikeStatus::Like;
                track
            })
            .collect();
        source.songs.extend(songs, page.next);
        source.loaded = true;
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn youtube_track(&self, id: &str) -> Option<&Track> {
        self.sources[0]
            .songs
            .iter()
            .find(|track| track.video_id == id)
            .or_else(|| self.removed_youtube.get(id))
    }

    pub(crate) fn spotify_liked(&self, id: &str) -> Option<bool> {
        if self.removed_spotify.contains(id) {
            Some(false)
        } else if self.source(Provider::Spotify).ids.contains(id) {
            Some(true)
        } else {
            None
        }
    }

    pub(crate) fn save_spotify_track(&mut self, mut track: Track) {
        self.removed_spotify.remove(&track.video_id);
        let spotify = self.source_mut(Provider::Spotify);
        if spotify.ids.insert(track.video_id.clone()) {
            track.in_library = true;
            track.like = LikeStatus::Like;
            track.set_video_id = None;
            spotify.songs.items.insert(0, track);
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub(crate) fn remove_spotify_track(&mut self, id: &str) {
        self.removed_spotify.insert(id.to_owned());
        let spotify = self.source_mut(Provider::Spotify);
        spotify.ids.remove(id);
        spotify.songs.items.retain(|track| track.video_id != id);
        if !spotify.complete() {
            spotify.generation = spotify.generation.wrapping_add(1);
            spotify.loaded = false;
            spotify.error = None;
            spotify.cursors.clear();
            spotify.songs.loading = false;
            spotify.songs.next = None;
        }
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn set_youtube_rating(&mut self, mut track: Track, like: LikeStatus) {
        if track.is_spotify() || track.local_path.is_some() {
            return;
        }
        track.like = like;
        let youtube = &mut self.sources[0];
        let id = track.video_id.clone();
        if like == LikeStatus::Like {
            self.removed_youtube.remove(&id);
            if youtube.ids.insert(id.clone()) {
                youtube.songs.items.insert(0, track);
            } else {
                return;
            }
        } else {
            self.removed_youtube.insert(id.clone(), track);
            if youtube.ids.remove(&id) {
                youtube.songs.items.retain(|track| track.video_id != id);
            } else {
                return;
            }
        }
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn set_library_state(&mut self, id: &str, in_library: bool) {
        if let Some(track) = self.sources[0]
            .songs
            .items
            .iter_mut()
            .find(|track| track.video_id == id)
            && track.in_library != in_library
        {
            track.in_library = in_library;
            self.revision = self.revision.wrapping_add(1);
        }
        if let Some(track) = self.removed_youtube.get_mut(id) {
            track.in_library = in_library;
        }
    }

    fn selected(&self, connected: &[Provider]) -> Vec<Provider> {
        connected
            .iter()
            .copied()
            .filter(|provider| self.filter.is_none_or(|filter| filter == *provider))
            .collect()
    }

    pub(crate) fn clear_errors(&mut self) {
        for source in &mut self.sources {
            source.error = None;
        }
    }

    fn refresh_rows(&mut self, selected: &[Provider], complete: bool, preferred: Provider) {
        let sort = if complete {
            self.sort
        } else {
            PlaylistSort::Default
        };
        let sources = SOURCES.map(|provider| selected.contains(&provider));
        let key = (self.revision, sources, sort, preferred);
        if self.projection == Some(key) {
            return;
        }
        let tracks: Vec<_> = selected
            .iter()
            .map(|provider| &self.source(*provider).songs)
            .collect();
        let longest = tracks
            .iter()
            .map(|tracks| tracks.len())
            .max()
            .unwrap_or_default();
        let entries: Vec<_> = (0..longest)
            .flat_map(|index| tracks.iter().filter_map(move |tracks| tracks.get(index)))
            .cloned()
            .map(Entry::Song)
            .collect();
        self.revision = self.revision.wrapping_add(1);
        self.rows
            .set_entries(&entries, self.revision, selected.len() > 1);
        self.rows.sort_playlist(sort, preferred);
        self.projection = Some((self.revision, sources, sort, preferred));
    }
}

pub(crate) fn paint_cover(ui: &egui::Ui, palette: &theme::Palette, rect: egui::Rect) {
    let radius = egui::CornerRadius::same((rect.width() * 0.085).round() as u8);
    ui.painter().rect_filled(rect, radius, palette.surface);
    let unit = rect.width();
    for (offset, fill) in [
        (-0.09, palette.surface_active),
        (0.09, palette.surface_hover),
    ] {
        let sleeve = egui::Rect::from_center_size(
            rect.center() + egui::vec2(unit * offset, unit * offset * 0.55),
            egui::Vec2::splat(unit * 0.55),
        );
        ui.painter().rect(
            sleeve,
            radius,
            fill,
            egui::Stroke::new(1.0, palette.outline),
            egui::StrokeKind::Inside,
        );
    }
    let size = unit * 0.30;
    crate::icons::Icon::HeartFilled
        .image(palette.accent_text(), size)
        .paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(size)),
        );
}

pub(crate) fn page(app: &mut App, ui: &mut egui::Ui) {
    let connected: Vec<_> = app
        .connected_providers()
        .into_iter()
        .filter(|p| SOURCES.contains(p))
        .collect();
    if app
        .liked_music
        .filter
        .is_some_and(|provider| !connected.contains(&provider))
    {
        app.liked_music.filter = None;
    }
    super::page_scroll(app, "liked-music").show(ui, |ui| {
        let state = &mut app.liked_music;
        let selected = state.selected(&connected);
        let complete = selected
            .iter()
            .all(|provider| state.source(*provider).complete());
        let preferred = app.settings.preferred_provider;
        state.refresh_rows(&selected, complete, preferred);
        let count = state.rows.len();
        let loading = selected
            .iter()
            .any(|provider| state.source(*provider).songs.loading);
        let sorting = !complete && state.sort != PlaylistSort::Default;
        let summary = match (count, complete && !connected.is_empty()) {
            (0, false) => None,
            (1, true) => Some("1 song".to_owned()),
            (count, true) => Some(format!("{count} songs")),
            (count, false) => Some(format!("{count} songs loaded")),
        };
        header(app, ui, summary.as_deref());
        controls(app, ui, &connected, &selected);
        ui.add_space(12.0);
        if connected.is_empty() {
            if matches!(app.auth, crate::auth::AuthState::Checking) {
                widgets::skeleton_track_rows(ui, &app.palette, 8, true);
            } else {
                ui.label("Connect a music provider to see your liked songs.");
                if widgets::outline_button(ui, &app.palette, "Manage providers").clicked() {
                    app.settings_tab = super::settings::SettingsTab::Providers;
                    app.navigate(Route::Settings);
                }
            }
            return;
        }
        for provider in &selected {
            if let Some(error) = app.liked_music.source(*provider).error.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Couldn't load {} songs", provider.name()))
                            .color(app.palette.secondary),
                    )
                    .on_hover_text(error);
                    if widgets::outline_button(ui, &app.palette, "Retry").clicked() {
                        app.liked_music.source_mut(*provider).error = None;
                        app.request_liked_music(*provider, true);
                    }
                });
            }
        }
        if count == 0 && loading {
            widgets::skeleton_track_rows(ui, &app.palette, 8, true);
        } else if count == 0 && complete {
            ui.label(egui::RichText::new("No liked songs yet.").color(app.palette.secondary));
        }
        if sorting && loading {
            ui.label(egui::RichText::new("Loading the rest to sort…").color(app.palette.secondary));
        }
        let mut rows = std::mem::take(&mut app.liked_music.rows);
        let at_end = count == 0 || source_rows::draw_playlist(app, ui, &mut rows, "liked-music");
        app.liked_music.rows = rows;
        if matches!(app.current_route(), Route::LikedMusic) && (at_end || sorting) {
            for provider in selected {
                app.request_liked_music(provider, true);
            }
            if at_end && count > 0 && !sorting && loading {
                ui.label(egui::RichText::new("Loading more songs…").color(app.palette.secondary));
            }
        }
    });
}

fn header(app: &mut App, ui: &mut egui::Ui, summary: Option<&str>) {
    let top = ui.cursor().top();
    let out = super::page_header(
        ui,
        &app.palette,
        &mut app.images,
        &app.backend,
        super::PageHeader {
            source: None,
            art_url: None,
            art_size: super::HEADER_ART_PT,
            circle: false,
            kind_label: "",
            title: TITLE,
            verification: None,
            description: None,
            artists: &[],
            second_plain: summary,
            meta: None,
            owner: None,
            credit_air: 4.0,
            play_size: None,
            playing: false,
            save_in_library: None,
            dots: false,
            salt: "liked-music",
        },
    );
    paint_cover(ui, &app.palette, out.art_rect);
    ui.add_space(out.art_bottom - top + 12.0);
}

fn controls(app: &mut App, ui: &mut egui::Ui, connected: &[Provider], selected: &[Provider]) {
    let playing = selected
        .iter()
        .any(|provider| app.is_playing_any(&app.liked_music.source(*provider).songs));
    ui.horizontal_wrapped(|ui| {
        if app.liked_music.rows.len() > 0 {
            if widgets::play_button(ui, &app.palette, playing, 48.0).clicked() {
                if playing {
                    app.toggle_playback();
                } else if let Some((tracks, start)) =
                    app.liked_music
                        .rows
                        .playback_queue(0, app.settings.preferred_provider, None)
                {
                    app.start_from(tracks, start);
                }
            }
            ui.add_space(4.0);
        }
        if connected.len() > 1 {
            for filter in [None, Some(Provider::YouTubeMusic), Some(Provider::Spotify)] {
                let label = filter.map_or("All", Provider::name);
                if widgets::chip(ui, &app.palette, label, app.liked_music.filter == filter)
                    .clicked()
                {
                    app.liked_music.filter = filter;
                    app.scroll_to_top = true;
                }
            }
        }
        if app.liked_music.rows.len() > 0 {
            let labels = PlaylistSort::ALL.map(PlaylistSort::label);
            if let Some(index) = widgets::icon_dropdown(
                ui,
                &app.palette,
                ui.id().with("liked-music-sort"),
                crate::icons::Icon::ArrowUpDown,
                false,
                app.liked_music.sort.label(),
                &labels,
            ) {
                app.liked_music.sort = PlaylistSort::ALL[index];
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str) -> Track {
        let mut track = crate::listening::Metadata {
            id: id.to_owned(),
            title: "Same song".to_owned(),
            ..Default::default()
        }
        .to_track();
        track.artists = vec![crate::model::ArtistLink::plain("Same artist")];
        track.duration = Some(180);
        track
    }

    #[test]
    fn one_song_keeps_both_sources_and_an_unlike_only_removes_youtube() {
        let [youtube, spotify] = SOURCES;
        let mut state = State::default();
        let yt = song("youtube-song");
        let sp = song("spotify:track:0000000000000000000001");
        state.loaded(
            youtube,
            Ok(Paged::new(vec![yt.clone()], Some("next".to_owned()))),
        );
        state.loaded(spotify, Ok(Paged::new(vec![sp.clone()], None)));
        state.refresh_rows(&SOURCES, false, youtube);
        assert_eq!(state.rows.len(), 1);
        assert_eq!(
            state
                .rows
                .playback_queue(0, youtube, Some(spotify))
                .unwrap()
                .0[0]
                .video_id,
            sp.video_id
        );

        state.set_youtube_rating(yt.clone(), LikeStatus::Indifferent);
        assert_eq!(
            state.youtube_track(&yt.video_id).unwrap().like,
            LikeStatus::Indifferent
        );
        state.loaded(youtube, Ok(Paged::new(vec![yt.clone()], None)));
        state.refresh_rows(&SOURCES, true, youtube);
        assert_eq!(state.rows.len(), 1);
        assert_eq!(
            state.rows.playback_queue(0, youtube, None).unwrap().0[0].video_id,
            sp.video_id
        );

        let rollback = state.youtube_track(&yt.video_id).unwrap().clone();
        state.set_youtube_rating(rollback, LikeStatus::Like);
        state.refresh_rows(&[youtube], true, spotify);
        assert_eq!(
            state.rows.playback_queue(0, spotify, None).unwrap().0[0].video_id,
            yt.video_id
        );
    }

    #[test]
    fn saved_spotify_track_stays_in_its_own_list_once() {
        let [youtube, spotify] = SOURCES;
        let mut state = State::default();
        let mut track = song("spotify:track:0000000000000000000001");
        track.set_video_id = Some("playlist-item".into());
        state.save_spotify_track(track.clone());
        state.loaded(spotify, Ok(Paged::new(vec![track], None)));

        assert!(state.source(youtube).songs.is_empty());
        assert_eq!(state.source(spotify).songs.len(), 1);
        let saved = &state.source(spotify).songs.items[0];
        assert_eq!(saved.like, LikeStatus::Like);
        assert!(saved.in_library);
        assert!(saved.set_video_id.is_none());
    }

    #[test]
    fn spotify_unlike_drops_the_track_and_restarts_offset_paging() {
        let [youtube, spotify] = SOURCES;
        let mut state = State::default();
        let removed = song("spotify:track:0000000000000000000001");
        let remaining = song("spotify:track:0000000000000000000002");
        state.loaded(youtube, Ok(Paged::new(vec![song("youtube-song")], None)));
        state.loaded(
            spotify,
            Ok(Paged::new(
                vec![removed.clone(), remaining.clone()],
                Some("next".into()),
            )),
        );
        assert_eq!(state.request(spotify, true), Some(Some("next".into())));
        let generation = state.generation(spotify);
        state.remove_spotify_track(&removed.video_id);
        assert_eq!(state.spotify_liked(&removed.video_id), Some(false));
        assert_ne!(state.generation(spotify), generation);
        assert_eq!(state.request(spotify, false), Some(None));
        state.loaded(
            spotify,
            Ok(Paged::new(vec![removed.clone(), remaining], None)),
        );
        assert_eq!(state.source(spotify).songs.len(), 1);
        assert_eq!(state.source(youtube).songs.len(), 1);
        state.save_spotify_track(removed);
        assert_eq!(state.source(spotify).songs.len(), 2);
    }

    #[test]
    fn a_repeated_cursor_keeps_loaded_songs_and_the_other_source() {
        let [youtube, spotify] = SOURCES;
        let mut state = State::default();
        assert_eq!(state.request(youtube, false), Some(None));
        assert_eq!(state.request(spotify, false), Some(None));
        state.loaded(
            youtube,
            Ok(Paged::new(vec![song("first")], Some("page-two".to_owned()))),
        );
        assert_eq!(
            state.request(youtube, true),
            Some(Some("page-two".to_owned()))
        );
        state.loaded(
            youtube,
            Ok(Paged::new(vec![song("first")], Some("page-two".to_owned()))),
        );
        assert_eq!(state.source(youtube).songs.len(), 1);
        assert!(state.source(youtube).error.is_some());
        assert_eq!(state.request(youtube, true), None);
        assert!(state.source(spotify).songs.loading);
    }
}
