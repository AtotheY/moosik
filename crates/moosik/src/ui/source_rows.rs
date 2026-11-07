use crate::app::{App, Route};
use crate::model::{Entry, Provider};
use crate::source_groups::{Group, entry_provider, group_entries};
use crate::ui::ListClick;
use crate::widgets;

#[derive(Clone, Copy)]
pub(crate) enum Playback {
    Single,
    List,
}

#[derive(Default)]
pub(crate) struct State {
    revision: Option<(u64, bool)>,
    groups: Vec<Group>,
    labels: Vec<String>,
    tracks: Vec<crate::model::Track>,
}

impl State {
    pub(crate) fn set_entries(&mut self, entries: &[Entry], revision: u64, enabled: bool) {
        if self.revision == Some((revision, enabled)) {
            return;
        }
        self.groups = group_entries(entries, enabled);
        self.labels = self
            .groups
            .iter()
            .map(|group| {
                group
                    .providers()
                    .into_iter()
                    .map(Provider::name)
                    .collect::<Vec<_>>()
                    .join(" · ")
            })
            .collect();
        self.revision = Some((revision, enabled));
    }

    pub(crate) fn set_tracks(&mut self, tracks: &[crate::model::Track], enabled: bool) {
        if self.tracks == tracks
            && self
                .revision
                .is_some_and(|(_, previous)| previous == enabled)
        {
            return;
        }
        self.tracks = tracks.to_vec();
        let entries: Vec<_> = tracks.iter().cloned().map(Entry::Song).collect();
        let revision = self
            .revision
            .map_or(0, |(revision, _)| revision)
            .wrapping_add(1);
        self.set_entries(&entries, revision, enabled);
    }

    fn selected(&self, group: &Group, preferred: Provider, now: Option<&str>) -> usize {
        let playing = now.and_then(|id| {
            group.variants.iter().find_map(|entry| {
                entry
                    .track()
                    .filter(|track| track.video_id == id)
                    .map(|_| entry_provider(entry))
            })
        });
        group.preferred_index(preferred, playing).unwrap_or(0)
    }

    pub(crate) fn playback_queue(
        &self,
        clicked: usize,
        preferred: Provider,
        explicit_source: Option<Provider>,
    ) -> Option<(Vec<crate::model::Track>, usize)> {
        let mut tracks = Vec::new();
        let mut clicked_track = None;
        for (index, group) in self.groups.iter().enumerate() {
            let source = explicit_source.filter(|_| index == clicked);
            if let Some(track) = group
                .preferred_entry(preferred, source)
                .and_then(Entry::track)
            {
                if index == clicked {
                    clicked_track = Some(tracks.len());
                }
                tracks.push(track.clone());
            }
        }
        clicked_track.map(|index| (tracks, index))
    }

    pub(crate) fn artist_profiles<'a>(
        &'a self,
        id: &'a str,
    ) -> impl Iterator<Item = (String, String)> + 'a {
        self.groups
            .iter()
            .filter_map(move |group| group.artist_profiles(id))
    }

    fn artist_route(&self, id: &str) -> Option<Route> {
        let mut profiles = self.artist_profiles(id);
        let (youtube, spotify) = profiles.next()?;
        if profiles.any(|pair| pair != (youtube.clone(), spotify.clone())) {
            return None;
        }
        Some(Route::ArtistGroup { youtube, spotify })
    }
}

fn combined_artist_route(group: &Group) -> Option<Route> {
    let Entry::Artist(youtube) = group.entry_for(Provider::YouTubeMusic)? else {
        return None;
    };
    let Entry::Artist(spotify) = group.entry_for(Provider::Spotify)? else {
        return None;
    };
    Some(Route::ArtistGroup {
        youtube: youtube.id.clone(),
        spotify: spotify.id.clone(),
    })
}

pub(crate) fn draw(
    app: &mut App,
    ui: &mut egui::Ui,
    state: &mut State,
    salt: &str,
    playback: Playback,
) -> bool {
    let preferred = app.settings.preferred_provider;
    let signed_in = app.signed_in();
    let now_id = app.now.track.as_ref().map(|track| track.video_id.as_str());
    let selected: Vec<_> = state
        .groups
        .iter()
        .map(|group| state.selected(group, preferred, now_id))
        .collect();
    let mut source_choice = None;
    let outcome = {
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
        crate::ui::spec_rows_with_slot(
            ui,
            palette,
            images,
            backend,
            stats,
            liked,
            state.groups.len(),
            |index| {
                let group = &state.groups[index];
                let mut spec = crate::ui::entry_row_spec(
                    &group.variants[selected[index]],
                    now,
                    library,
                    signed_in,
                );
                if group.is_combined()
                    && matches!(
                        group.variants[selected[index]],
                        Entry::Song(_) | Entry::Video(_)
                    )
                    && spec.duration.is_none_or(|duration| duration == 0)
                {
                    spec.duration = group
                        .variants
                        .iter()
                        .filter_map(Entry::track)
                        .find_map(|track| track.duration.filter(|duration| *duration > 0));
                }
                if group.is_combined()
                    && matches!(group.variants[selected[index]], Entry::Artist(_))
                {
                    spec.subtitle = widgets::plain_subtitle(&state.labels[index]);
                }
                if group.is_combined()
                    && group.variants.iter().filter_map(Entry::track).any(|track| {
                        now.track
                            .as_ref()
                            .is_some_and(|playing| playing.video_id == track.video_id)
                    })
                {
                    spec.active = true;
                    spec.playing = !now.paused;
                }
                spec
            },
            |index| {
                let group = &state.groups[index];
                match &group.variants[selected[index]] {
                    Entry::Song(_) | Entry::Video(_) | Entry::Album(_) => {
                        let count = group.providers().len() as f32;
                        widgets::RowSlot::Subtitle(count * 24.0 + (count - 1.0) * 4.0)
                    }
                    Entry::Artist(_) if group.is_combined() => widgets::RowSlot::Trailing(120.0),
                    _ => widgets::RowSlot::Subtitle(24.0),
                }
            },
            |ui, index| {
                let group = &state.groups[index];
                let entry = &group.variants[selected[index]];
                let provider = entry_provider(entry);
                if group.is_combined()
                    && matches!(entry, Entry::Song(_) | Entry::Video(_) | Entry::Album(_))
                {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for candidate in group.providers() {
                        if widgets::provider_button(
                            ui,
                            palette,
                            candidate,
                            candidate == provider,
                            if entry.track().is_some() {
                                "Play from"
                            } else {
                                "Open on"
                            },
                        )
                        .clicked()
                        {
                            source_choice = Some((index, candidate));
                        }
                    }
                } else if group.is_combined() && matches!(entry, Entry::Artist(_)) {
                    let providers = group.providers();
                    let labels: Vec<_> = providers
                        .iter()
                        .map(|provider| format!("Open {}", provider.name()))
                        .collect();
                    let options: Vec<_> = labels.iter().map(String::as_str).collect();
                    let current = providers
                        .iter()
                        .position(|candidate| *candidate == provider)
                        .map(|index| labels[index].as_str())
                        .unwrap_or("");
                    if let Some(choice) = widgets::text_dropdown_with_label(
                        ui,
                        palette,
                        ui.id().with((salt, &group.key)),
                        "Profiles",
                        current,
                        &options,
                    ) {
                        source_choice = Some((index, providers[choice]));
                    }
                } else {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::Vec2::splat(24.0), egui::Sense::hover());
                    widgets::source_name_indicator(
                        ui,
                        palette,
                        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(20.0)),
                        provider.name(),
                        response.id.with("source"),
                    );
                }
            },
        )
    };
    let at_end = outcome.at_end;
    if let Some(action) = outcome.action {
        app.row_action(action);
    }
    if let Some((index, provider)) = source_choice {
        activate(app, state, index, Some(provider), playback);
        return at_end;
    }
    match outcome.click {
        Some(ListClick::Row(index)) => activate(app, state, index, None, playback),
        Some(ListClick::Link(Route::Artist(id))) => {
            app.navigate(state.artist_route(&id).unwrap_or(Route::Artist(id)));
        }
        Some(ListClick::Link(route)) => app.navigate(route),
        None => {}
    }
    at_end
}

fn activate(
    app: &mut App,
    state: &State,
    index: usize,
    explicit_source: Option<Provider>,
    playback: Playback,
) {
    let preferred = app.settings.preferred_provider;
    let group = &state.groups[index];
    let selected = group
        .preferred_index(preferred, explicit_source)
        .unwrap_or(0);
    match &group.variants[selected] {
        Entry::Song(_) | Entry::Video(_) => {
            let Some((queue, queue_index)) =
                state.playback_queue(index, preferred, explicit_source)
            else {
                return;
            };
            if explicit_source.is_some() {
                if matches!(playback, Playback::Single) && queue[queue_index].local_path.is_none() {
                    app.play_source(vec![queue[queue_index].clone()], 0);
                } else {
                    app.play_source(queue, queue_index);
                }
            } else {
                match playback {
                    Playback::Single => app.play_row(queue, queue_index),
                    Playback::List => app.start_from(queue, queue_index),
                }
            }
        }
        Entry::Artist(artist) => {
            let route = if explicit_source.is_some() {
                None
            } else {
                combined_artist_route(group)
            };
            app.navigate(route.unwrap_or_else(|| Route::Artist(artist.id.clone())));
        }
        Entry::Album(album) => app.navigate(Route::Album(album.id.clone())),
        Entry::Playlist(playlist) => app.navigate(Route::Playlist(playlist.id.clone())),
        Entry::Mood(mood) => app.navigate(Route::Mood(mood.params.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mixed_rows() -> State {
        let mut entries = vec![Entry::Artist(crate::model::ArtistRef {
            id: "UCartist".to_owned(),
            name: "Artist".to_owned(),
            subscribers: None,
            thumbnail: None,
            actions: crate::model::ArtistActions::default(),
        })];
        for (index, title) in ["First song", "Second song"].into_iter().enumerate() {
            for provider in [Provider::YouTubeMusic, Provider::Spotify] {
                let mut track = crate::listening::Metadata {
                    id: format!("youtube{index}"),
                    title: title.to_owned(),
                    ..Default::default()
                }
                .to_track();
                track.artists = vec![crate::model::ArtistLink::plain("Artist")];
                track.duration = Some(180);
                if provider == Provider::Spotify {
                    track.video_id = format!("spotify:track:{:022}", index + 1);
                }
                entries.push(Entry::Song(track));
            }
        }
        let mut state = State::default();
        state.set_entries(&entries, 1, true);
        state
    }

    #[test]
    fn cached_tracks_rebuild_when_metadata_order_or_grouping_changes() {
        let source = mixed_rows();
        let tracks: Vec<_> = source
            .groups
            .iter()
            .flat_map(|group| group.variants.iter())
            .filter_map(Entry::track)
            .cloned()
            .collect();
        let mut state = State::default();
        state.set_tracks(&tracks, true);
        assert_eq!(state.groups.len(), 2);
        let revision = state.revision;
        state.set_tracks(&tracks, true);
        assert_eq!(state.revision, revision);
        let mut changed = tracks.clone();
        changed[0].title = "Different edition".to_owned();
        state.set_tracks(&changed, true);
        assert_eq!(state.groups.len(), 3);
        changed.reverse();
        state.set_tracks(&changed, true);
        assert_eq!(state.groups[0].variants[0].track(), Some(&changed[0]));
        state.set_tracks(&tracks, false);
        assert_eq!(state.groups.len(), tracks.len());
        state.set_tracks(&[], false);
        assert!(state.groups.is_empty());
    }

    #[test]
    fn normal_play_uses_preference_even_when_the_other_source_is_playing() {
        let state = mixed_rows();
        assert_eq!(state.groups.len(), 3);
        for preferred in [Provider::YouTubeMusic, Provider::Spotify] {
            let other = if preferred == Provider::Spotify {
                Provider::YouTubeMusic
            } else {
                Provider::Spotify
            };
            let group = &state.groups[1];
            let playing = group.entry_for(other).unwrap().track().unwrap();
            let displayed = state.selected(group, preferred, Some(&playing.video_id));
            assert_eq!(entry_provider(&group.variants[displayed]), other);
            let (queue, index) = state.playback_queue(1, preferred, None).unwrap();
            assert_eq!(index, 0);
            assert_eq!(queue.len(), 2);
            assert!(queue.iter().all(|track| track.provider() == preferred));
            assert_eq!(
                queue[index].video_id,
                group
                    .entry_for(preferred)
                    .unwrap()
                    .track()
                    .unwrap()
                    .video_id
            );
        }
    }

    #[test]
    fn explicit_source_only_overrides_the_clicked_song_and_does_not_stick() {
        let state = mixed_rows();
        let (queue, index) = state
            .playback_queue(2, Provider::Spotify, Some(Provider::YouTubeMusic))
            .unwrap();
        assert_eq!(index, 1);
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].provider(), Provider::Spotify);
        assert_eq!(queue[1].provider(), Provider::YouTubeMusic);
        let (queue, index) = state.playback_queue(2, Provider::Spotify, None).unwrap();
        assert_eq!(index, 1);
        assert!(queue.iter().all(crate::model::Track::is_spotify));
    }

    #[test]
    fn changed_preference_applies_without_rebuilding_the_results() {
        let state = mixed_rows();
        let (youtube, _) = state
            .playback_queue(1, Provider::YouTubeMusic, None)
            .unwrap();
        let (spotify, _) = state.playback_queue(1, Provider::Spotify, None).unwrap();
        assert!(youtube.iter().all(|track| !track.is_spotify()));
        assert!(spotify.iter().all(crate::model::Track::is_spotify));
        assert_eq!(state.revision, Some((1, true)));
    }

    #[test]
    fn unavailable_preference_uses_the_available_source() {
        let state = mixed_rows();
        let spotify = state.groups[1]
            .entry_for(Provider::Spotify)
            .unwrap()
            .clone();
        let mut single = State::default();
        single.set_entries(&[spotify], 1, false);
        let (queue, index) = single
            .playback_queue(0, Provider::YouTubeMusic, None)
            .unwrap();
        assert_eq!(index, 0);
        assert_eq!(queue.len(), 1);
        assert!(queue[0].is_spotify());
        assert!(state.playback_queue(0, Provider::Spotify, None).is_none());
    }
}
