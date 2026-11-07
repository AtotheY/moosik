use std::collections::{HashMap, HashSet, VecDeque};

use crate::model::{Entry, HomeSection, PlaylistRef, PlaylistSource, Provider, Shelf, Track};

pub fn shelf_provider(shelf: &Shelf) -> Option<Provider> {
    let mut providers = shelf.items.iter().map(crate::source_groups::entry_provider);
    let provider = providers.next()?;
    providers.all(|other| other == provider).then_some(provider)
}

pub fn local_shelves(tracks: &[Track], playlists: &[PlaylistRef]) -> Vec<Shelf> {
    let mut shelves = Vec::new();
    let songs = tracks
        .iter()
        .filter(|track| track.provider() == Provider::Local)
        .take(24)
        .cloned()
        .map(Entry::Song)
        .collect::<Vec<_>>();
    if !songs.is_empty() {
        shelves.push(Shelf {
            title: "Local songs".to_owned(),
            strapline: Some("Local".to_owned()),
            items: songs,
            more_params: None,
            more_browse_id: None,
            expanded: false,
        });
    }
    let playlists = playlists
        .iter()
        .filter(|playlist| playlist.source == PlaylistSource::Local)
        .take(12)
        .cloned()
        .map(Entry::Playlist)
        .collect::<Vec<_>>();
    if !playlists.is_empty() {
        shelves.push(Shelf {
            title: "Local playlists".to_owned(),
            strapline: Some("Local".to_owned()),
            items: playlists,
            more_params: None,
            more_browse_id: None,
            expanded: false,
        });
    }
    shelves
}

pub fn arrange(
    shelves: &mut Vec<Shelf>,
    order: &[HomeSection],
    combined: bool,
    primary: Option<Provider>,
) {
    if !combined {
        crate::model::order_home(shelves, order);
        return;
    }
    let mut providers: Vec<_> = Provider::ALL.into_iter().map(Some).collect();
    if let Some(primary) = primary {
        providers.retain(|provider| *provider != Some(primary));
        providers.insert(0, Some(primary));
    }
    providers.push(None);
    let mut buckets: Vec<Vec<Shelf>> = providers.iter().map(|_| Vec::new()).collect();
    for shelf in std::mem::take(shelves) {
        let source = shelf_provider(&shelf);
        let index = providers
            .iter()
            .position(|provider| *provider == source)
            .unwrap_or(providers.len() - 1);
        buckets[index].push(shelf);
    }
    let mut buckets: Vec<VecDeque<Shelf>> = buckets
        .into_iter()
        .map(|mut bucket| {
            crate::model::order_home(&mut bucket, order);
            bucket.into()
        })
        .collect();
    while buckets.iter().any(|bucket| !bucket.is_empty()) {
        for (index, bucket) in buckets.iter_mut().enumerate() {
            let weight = if primary.is_some() && providers[index] == primary {
                2
            } else {
                1
            };
            for _ in 0..weight {
                if let Some(shelf) = bucket.pop_front() {
                    shelves.push(shelf);
                }
            }
        }
    }
}

pub(crate) enum DisplayShelf {
    Native,
    Merged(Shelf),
    Hidden,
}

pub(crate) fn merge_categories(shelves: &[Shelf], primary: Option<Provider>) -> Vec<DisplayShelf> {
    let mut groups: HashMap<_, Vec<_>> = HashMap::new();
    for (index, shelf) in shelves.iter().enumerate() {
        if let Some(key) = category_key(shelf)
            && let Some(provider) = shelf_provider(shelf)
        {
            groups.entry(key).or_default().push((index, provider));
        }
    }
    let mut display: Vec<_> = shelves.iter().map(|_| DisplayShelf::Native).collect();
    for ((title, _, _), members) in groups {
        let mut providers = HashSet::new();
        if members.len() < 2
            || !members
                .iter()
                .all(|&(_, provider)| providers.insert(provider))
        {
            continue;
        }
        let index = members[0].0;
        let first = &shelves[index];
        let mut items = members
            .iter()
            .flat_map(|&(index, _)| shelves[index].items.iter().cloned())
            .collect();
        crate::source_groups::prioritize_entries(&mut items, primary);
        display[index] = DisplayShelf::Merged(Shelf {
            title: if title == "new releases" {
                HomeSection::NewReleases.label().to_owned()
            } else {
                first.title.clone()
            },
            strapline: first.strapline.clone(),
            items,
            more_params: None,
            more_browse_id: None,
            expanded: false,
        });
        for &(index, _) in &members[1..] {
            display[index] = DisplayShelf::Hidden;
        }
    }
    display
}

fn category_key(shelf: &Shelf) -> Option<(String, &'static str, &str)> {
    let kind = shelf.items.first()?.kind();
    if kind == "Mood" || !shelf.items.iter().all(|entry| entry.kind() == kind) {
        return None;
    }
    let mut title = shelf.title.trim().to_lowercase();
    match shelf.home_section() {
        HomeSection::NewReleases if kind == "Album" => title = "new releases".to_owned(),
        HomeSection::NewReleases | HomeSection::Themed | HomeSection::SimilarTo => return None,
        HomeSection::Everything
            if !matches!(title.as_str(), "made for you" | "recently played") =>
        {
            return None;
        }
        _ => {}
    }
    let strapline = shelf.strapline.as_deref().unwrap_or_default().trim();
    Some((title, kind, strapline))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(provider: Provider, label: &str) -> Track {
        let mut track = crate::listening::Metadata {
            id: label.to_owned(),
            title: label.to_owned(),
            ..Default::default()
        }
        .to_track();
        match provider {
            Provider::YouTubeMusic => {}
            Provider::Spotify => track.video_id = format!("spotify:track:{label:0<22}"),
            Provider::Local => track.local_path = Some(std::path::PathBuf::from(label).into()),
        }
        track
    }

    fn shelf(provider: Provider, title: &str, label: &str) -> Shelf {
        Shelf {
            title: title.to_owned(),
            strapline: None,
            items: vec![Entry::Song(track(provider, label))],
            more_params: None,
            more_browse_id: None,
            expanded: false,
        }
    }

    fn labels(shelves: &[Shelf]) -> Vec<&str> {
        shelves
            .iter()
            .map(|shelf| {
                shelf
                    .items
                    .first()
                    .map(Entry::title)
                    .unwrap_or(&shelf.title)
            })
            .collect()
    }

    #[test]
    fn release_aliases_merge_in_place_with_primary_entries_first() {
        let album = |id: &str, title: &str| {
            Entry::Album(crate::model::AlbumRef {
                id: id.to_owned(),
                title: title.to_owned(),
                artist: String::new(),
                artists: Vec::new(),
                explicit: None,
                year: None,
                kind: None,
                thumbnail: None,
            })
        };
        let mut youtube = shelf(Provider::YouTubeMusic, "New albums & singles", "yt");
        youtube.items = vec![album("MPREyt", "YouTube album")];
        youtube.more_params = Some("youtube-more".into());
        let mut spotify = shelf(Provider::Spotify, "New releases", "sp");
        spotify.items = vec![album(
            "spotify:album:0000000000000000000000",
            "Spotify album",
        )];
        let native = vec![
            youtube,
            shelf(Provider::YouTubeMusic, "Quick picks", "unrelated"),
            spotify,
        ];
        let display = merge_categories(&native, Some(Provider::Spotify));
        let [
            DisplayShelf::Merged(releases),
            DisplayShelf::Native,
            DisplayShelf::Hidden,
        ] = display.as_slice()
        else {
            panic!("Expected one combined release shelf");
        };
        assert_eq!(releases.title, "New releases");
        assert_eq!(
            releases.items,
            [native[2].items[0].clone(), native[0].items[0].clone()]
        );
        assert!(releases.more_params.is_none());
        assert_eq!(native[0].more_params.as_deref(), Some("youtube-more"));
    }

    #[test]
    fn equivalent_song_shelves_preserve_variants_and_reject_ambiguity() {
        let youtube = shelf(Provider::YouTubeMusic, "Quick picks", "yt");
        let spotify = shelf(Provider::Spotify, "quick picks", "sp");
        let display = merge_categories(&[youtube.clone(), spotify.clone()], None);
        let [DisplayShelf::Merged(picks), DisplayShelf::Hidden] = display.as_slice() else {
            panic!("Expected one combined Quick picks shelf");
        };
        assert_eq!(
            picks.items,
            [youtube.items[0].clone(), spotify.items[0].clone()]
        );

        let mut account_shelf = spotify.clone();
        account_shelf.strapline = Some("For this account".into());
        let mut video_shelf = spotify.clone();
        video_shelf.items = vec![Entry::Video(track(Provider::Spotify, "video"))];
        for native in [
            vec![youtube.clone(), account_shelf],
            vec![youtube.clone(), video_shelf],
            vec![youtube.clone(), youtube, spotify],
            vec![
                shelf(Provider::YouTubeMusic, "Fresh finds", "yt"),
                shelf(Provider::Spotify, "Fresh finds", "sp"),
            ],
        ] {
            assert!(
                merge_categories(&native, None)
                    .iter()
                    .all(|slot| matches!(slot, DisplayShelf::Native))
            );
        }
    }

    #[test]
    fn balanced_home_interleaves_sources_without_changing_their_rank() {
        let mut shelves = vec![
            shelf(Provider::Local, "Local second", "local2"),
            shelf(Provider::YouTubeMusic, "YouTube second", "yt2"),
            shelf(Provider::Spotify, "Spotify first", "sp1"),
            shelf(Provider::YouTubeMusic, "YouTube first", "yt1"),
            shelf(Provider::Local, "Local first", "local1"),
            shelf(Provider::Spotify, "Spotify second", "sp2"),
        ];
        arrange(&mut shelves, &[HomeSection::Everything], true, None);
        assert_eq!(
            labels(&shelves),
            ["yt2", "sp1", "local2", "yt1", "sp2", "local1"]
        );
    }

    #[test]
    fn primary_home_places_two_primary_shelves_before_one_from_each_other_source() {
        let mut shelves = vec![
            shelf(Provider::YouTubeMusic, "YT first", "yt1"),
            shelf(Provider::Spotify, "SP first", "sp1"),
            shelf(Provider::Local, "Local first", "local1"),
            shelf(Provider::Spotify, "SP second", "sp2"),
            shelf(Provider::YouTubeMusic, "YT second", "yt2"),
            shelf(Provider::Spotify, "SP third", "sp3"),
            shelf(Provider::Local, "Local second", "local2"),
            shelf(Provider::Spotify, "SP fourth", "sp4"),
            shelf(Provider::YouTubeMusic, "YT third", "yt3"),
            shelf(Provider::Spotify, "SP fifth", "sp5"),
        ];
        arrange(
            &mut shelves,
            &[HomeSection::Everything],
            true,
            Some(Provider::Spotify),
        );
        assert_eq!(
            labels(&shelves),
            [
                "sp1", "sp2", "yt1", "local1", "sp3", "sp4", "yt2", "local2", "sp5", "yt3"
            ]
        );
    }

    #[test]
    fn custom_section_order_applies_inside_each_provider_before_mixing() {
        let mut shelves = vec![
            shelf(Provider::YouTubeMusic, "Listen again", "ytlisten"),
            shelf(Provider::Spotify, "Other Spotify", "spother"),
            shelf(Provider::Local, "Listen again", "locallisten"),
            shelf(Provider::Spotify, "Listen again", "splisten"),
            shelf(Provider::YouTubeMusic, "Quick picks", "ytquick"),
            shelf(Provider::Spotify, "Quick picks", "spquick"),
            shelf(Provider::Local, "Quick picks", "localquick"),
            shelf(Provider::YouTubeMusic, "Other YouTube", "ytother"),
        ];
        arrange(
            &mut shelves,
            &[
                HomeSection::QuickPicks,
                HomeSection::ListenAgain,
                HomeSection::Everything,
            ],
            true,
            None,
        );
        assert_eq!(
            labels(&shelves),
            [
                "ytquick",
                "spquick",
                "localquick",
                "ytlisten",
                "splisten",
                "locallisten",
                "ytother",
                "spother"
            ]
        );
    }

    #[test]
    fn local_home_classifies_the_playback_source_and_filters_remote_content() {
        let mut local = track(Provider::Local, "local");
        local.video_id = "spotify:track:3TO7bbrUKrOSPGRTB5MeCz".to_owned();
        let mut playlist = PlaylistRef {
            id: "private:list".to_owned(),
            title: "Private playlist".to_owned(),
            author: String::new(),
            author_id: None,
            track_count: None,
            thumbnail: None,
            editable: true,
            source: PlaylistSource::Local,
        };
        let local_playlist = playlist.clone();
        playlist.source = PlaylistSource::Spotify;
        let shelves = local_shelves(
            &[
                track(Provider::YouTubeMusic, "youtube"),
                track(Provider::Spotify, "spotify"),
                local.clone(),
            ],
            &[playlist, local_playlist],
        );
        assert_eq!(shelves.len(), 2);
        assert!(
            shelves
                .iter()
                .all(|shelf| shelf_provider(shelf) == Some(Provider::Local))
        );
        assert_eq!(shelves[0].items, [Entry::Song(local)]);
        assert_eq!(shelves[1].items.len(), 1);
        let mut mixed = shelf(Provider::YouTubeMusic, "Mixed", "youtube");
        mixed
            .items
            .push(Entry::Song(track(Provider::Spotify, "spotify")));
        assert_eq!(shelf_provider(&mixed), None);
        mixed.items.clear();
        assert_eq!(shelf_provider(&mixed), None);
    }

    #[test]
    fn every_shelf_survives_once_including_mixed_empty_and_duplicate_shelves() {
        let duplicate = shelf(Provider::YouTubeMusic, "Duplicate", "duplicate");
        let mut mixed = shelf(Provider::YouTubeMusic, "Mixed", "mixedyt");
        mixed
            .items
            .push(Entry::Song(track(Provider::Spotify, "mixedspotify")));
        let mut empty = shelf(Provider::Local, "Empty", "empty");
        empty.items.clear();
        let original = vec![
            duplicate.clone(),
            shelf(Provider::Spotify, "Spotify", "spotify"),
            mixed,
            shelf(Provider::Local, "Local", "local"),
            duplicate,
            empty,
        ];
        for primary in [
            None,
            Some(Provider::YouTubeMusic),
            Some(Provider::Spotify),
            Some(Provider::Local),
        ] {
            let mut shelves = original.clone();
            arrange(&mut shelves, &[HomeSection::Everything], true, primary);
            assert_eq!(shelves.len(), original.len());
            for shelf in &original {
                assert_eq!(
                    shelves
                        .iter()
                        .filter(|candidate| *candidate == shelf)
                        .count(),
                    original
                        .iter()
                        .filter(|candidate| *candidate == shelf)
                        .count()
                );
            }
        }
    }

    #[test]
    fn missing_primary_source_falls_back_to_balanced_available_sources() {
        for absent in Provider::ALL {
            let original: Vec<_> = Provider::ALL
                .into_iter()
                .filter(|provider| *provider != absent)
                .flat_map(|provider| {
                    [
                        shelf(provider, "First", "first"),
                        shelf(provider, "Second", "second"),
                        shelf(provider, "Third", "third"),
                    ]
                })
                .collect();
            let mut balanced = original.clone();
            arrange(&mut balanced, &[HomeSection::Everything], true, None);
            let mut preferred = original;
            arrange(
                &mut preferred,
                &[HomeSection::Everything],
                true,
                Some(absent),
            );
            assert_eq!(preferred, balanced);
        }
    }

    #[test]
    fn single_source_mode_uses_global_section_order_without_weighting() {
        let mut shelves = vec![
            shelf(Provider::YouTubeMusic, "Listen again", "listen"),
            shelf(Provider::YouTubeMusic, "Other", "other"),
            shelf(Provider::YouTubeMusic, "Quick picks", "quick"),
        ];
        arrange(
            &mut shelves,
            &[
                HomeSection::QuickPicks,
                HomeSection::ListenAgain,
                HomeSection::Everything,
            ],
            false,
            Some(Provider::Spotify),
        );
        assert_eq!(labels(&shelves), ["quick", "listen", "other"]);
    }
}
