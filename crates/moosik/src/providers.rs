use std::{collections::HashSet, future::Future};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::api::{Api, LibrarySection, LibrarySortParam};
use crate::innertube::Page;
use crate::model::{Entry, HomeFeed, HomeFetch, Library, Paged, Provider, Shelf, Track};

const CURSOR_PREFIX: &str = "providers:";

#[derive(Default, Serialize, Deserialize)]
struct Cursor {
    youtube: Option<String>,
    spotify: Option<String>,
    spotify_next: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    YouTubeMusic,
    Spotify,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Self::YouTubeMusic => "YouTube Music",
            Self::Spotify => "Spotify",
        }
    }
}

impl Cursor {
    fn order(&self) -> [Source; 2] {
        if self.spotify_next {
            [Source::Spotify, Source::YouTubeMusic]
        } else {
            [Source::YouTubeMusic, Source::Spotify]
        }
    }

    fn token(&self, source: Source) -> Option<&str> {
        match source {
            Source::YouTubeMusic => self.youtube.as_deref(),
            Source::Spotify => self.spotify.as_deref(),
        }
    }

    fn advance(&mut self, source: Source, next: Option<String>) -> Result<()> {
        ensure!(
            next.as_deref()
                .is_none_or(|next| Some(next) != self.token(source)),
            "Library continuation did not advance"
        );
        match source {
            Source::YouTubeMusic => self.youtube = next,
            Source::Spotify => self.spotify = next,
        }
        self.spotify_next = source == Source::YouTubeMusic;
        Ok(())
    }

    fn encode(self) -> Option<String> {
        if self.youtube.is_none() && self.spotify.is_none() {
            return None;
        }
        Some(format!(
            "{CURSOR_PREFIX}{}",
            serde_json::to_string(&self).expect("cursor serialization")
        ))
    }

    fn decode(token: &str) -> Result<Self> {
        match token.strip_prefix(CURSOR_PREFIX) {
            Some(json) => serde_json::from_str(json).context("Invalid library continuation"),
            None => Ok(Self {
                youtube: Some(token.to_owned()),
                ..Default::default()
            }),
        }
    }
}

#[derive(Debug)]
pub struct Loaded<T> {
    pub value: T,
    pub warnings: Vec<String>,
}

pub struct LibraryLoad {
    pub value: Library,
    pub failed_provider: Option<(Provider, String)>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HomeSources {
    pub youtube: bool,
    pub spotify: bool,
}

fn merge_page<T>(mut youtube: Paged<T>, spotify: Paged<T>) -> Paged<T> {
    youtube.items.extend(spotify.items);
    youtube.next = Cursor {
        youtube: youtube.next,
        spotify: spotify.next,
        spotify_next: false,
    }
    .encode();
    youtube.loading = false;
    youtube
}

fn merge_library(youtube: Library, spotify: Library) -> Library {
    Library {
        songs: merge_page(youtube.songs, spotify.songs),
        artists: merge_page(youtube.artists, spotify.artists),
        albums: merge_page(youtube.albums, spotify.albums),
        playlists: merge_page(youtube.playlists, spotify.playlists),
    }
}

fn collect<T: Default>(
    youtube: Option<Result<T>>,
    spotify: Option<Result<T>>,
) -> Result<(T, T, Vec<String>)> {
    let mut warnings = Vec::new();
    let mut successes = 0;
    let mut take = |result: Option<Result<T>>, name| match result {
        Some(Ok(value)) => {
            successes += 1;
            value
        }
        Some(Err(error)) => {
            warnings.push(format!("{name}: {error:#}"));
            T::default()
        }
        None => T::default(),
    };
    let youtube = take(youtube, "YouTube Music");
    let spotify = take(spotify, "Spotify");
    if successes == 0 {
        bail!(
            "{}",
            if warnings.is_empty() {
                "Sign in to see your library".to_owned()
            } else {
                warnings.join("\n")
            }
        );
    }
    Ok((youtube, spotify, warnings))
}

pub async fn library(
    api: &Api,
    sort: LibrarySortParam,
    youtube: bool,
    spotify: bool,
) -> Result<LibraryLoad> {
    let sort_merged_songs = spotify && sort != LibrarySortParam::RecentlySaved;
    let (youtube, spotify) = tokio::join!(
        async {
            if youtube {
                Some(api.library(sort).await)
            } else {
                None
            }
        },
        async {
            if spotify {
                Some(crate::spotify::library_with_warnings().await)
            } else {
                None
            }
        },
    );
    let (spotify, section_warnings) = match spotify {
        Some(Ok((library, warnings))) => (Some(Ok(library)), warnings),
        Some(Err(error)) => (Some(Err(error)), Vec::new()),
        None => (None, Vec::new()),
    };
    let mut loaded = assemble_library(youtube, spotify, section_warnings)?;
    if sort_merged_songs {
        if let Some((provider, error)) = &loaded.failed_provider {
            bail!(
                "Could not load the complete library for sorting:\n{}: {error}",
                provider.name()
            );
        }
        loaded.value.songs = sorted_songs(loaded.value.songs, sort, |token| async move {
            library_more(api, LibrarySection::Songs, &token).await
        })
        .await?;
    }
    Ok(loaded)
}

fn assemble_library(
    youtube: Option<Result<Library>>,
    spotify: Option<Result<Library>>,
    warnings: Vec<String>,
) -> Result<LibraryLoad> {
    let failed_provider = [
        (Provider::YouTubeMusic, youtube.as_ref()),
        (Provider::Spotify, spotify.as_ref()),
    ]
    .into_iter()
    .find_map(|(provider, result)| {
        result?
            .as_ref()
            .err()
            .map(|error| (provider, format!("{error:#}")))
    });
    let (youtube, spotify, _) = collect(youtube, spotify)?;
    Ok(LibraryLoad {
        value: merge_library(youtube, spotify),
        failed_provider,
        warnings,
    })
}

pub(crate) async fn retry_library_provider(
    api: &Api,
    provider: Provider,
    sort: LibrarySortParam,
) -> Result<Loaded<Library>> {
    let (value, warnings) = match provider {
        Provider::YouTubeMusic => (api.library(sort).await?, Vec::new()),
        Provider::Spotify => crate::spotify::library_with_warnings().await?,
        Provider::Local => bail!("Local music does not use the online library"),
    };
    Ok(Loaded { value, warnings })
}

pub(crate) fn merge_recovered_library(
    library: &mut Library,
    recovered: Library,
    provider: Provider,
) {
    let source = match provider {
        Provider::YouTubeMusic => Source::YouTubeMusic,
        Provider::Spotify => Source::Spotify,
        Provider::Local => return,
    };
    fn append<T>(
        page: &mut Paged<T>,
        recovered: Paged<T>,
        source: Source,
        id: impl Fn(&T) -> &str,
    ) {
        let mut cursor = page
            .next
            .as_deref()
            .and_then(|token| Cursor::decode(token).ok())
            .unwrap_or_default();
        match source {
            Source::YouTubeMusic => cursor.youtube = recovered.next,
            Source::Spotify => cursor.spotify = recovered.next,
        }
        cursor.spotify_next = source == Source::Spotify;
        let ids: HashSet<_> = page.items.iter().map(&id).collect();
        let items = recovered
            .items
            .into_iter()
            .filter(|item| !ids.contains(id(item)))
            .collect();
        page.extend(items, cursor.encode());
    }
    append(&mut library.songs, recovered.songs, source, |track| {
        &track.video_id
    });
    append(&mut library.artists, recovered.artists, source, |artist| {
        &artist.id
    });
    append(&mut library.albums, recovered.albums, source, |album| {
        &album.id
    });
    append(
        &mut library.playlists,
        recovered.playlists,
        source,
        |playlist| &playlist.id,
    );
}

async fn sorted_songs<F, Fut>(
    mut songs: Paged<Track>,
    sort: LibrarySortParam,
    mut more: F,
) -> Result<Paged<Track>>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Page>>,
{
    let mut seen = HashSet::new();
    while let Some(token) = songs.next.clone() {
        ensure!(
            seen.insert(token.clone()),
            "Library continuation repeated while loading the complete collection"
        );
        let page = more(token)
            .await
            .context("Could not load the complete library for sorting")?;
        songs.extend(
            page.items
                .into_iter()
                .filter_map(|entry| entry.track().cloned())
                .collect(),
            page.next,
        );
    }
    match sort {
        LibrarySortParam::NameAsc => songs
            .items
            .sort_by_cached_key(|track| track.title.to_lowercase()),
        LibrarySortParam::NameDesc => songs
            .items
            .sort_by_cached_key(|track| std::cmp::Reverse(track.title.to_lowercase())),
        LibrarySortParam::RecentlySaved => {}
    }
    Ok(songs)
}

pub async fn sidebar_playlists(api: &Api, youtube: bool, spotify: bool) -> Result<Loaded<Page>> {
    let (youtube, spotify) = tokio::join!(
        async {
            if youtube {
                Some(api.library_playlists().await)
            } else {
                None
            }
        },
        async {
            if spotify {
                Some(crate::spotify::sidebar_playlists().await)
            } else {
                None
            }
        },
    );
    let (youtube, spotify, warnings) = collect(youtube, spotify)?;
    let page = merge_page(
        Paged::new(youtube.items, youtube.next),
        Paged::new(spotify.items, spotify.next),
    );
    Ok(Loaded {
        value: Page {
            items: page.items,
            next: page.next,
        },
        warnings,
    })
}

pub async fn library_more(api: &Api, section: LibrarySection, token: &str) -> Result<Page> {
    continued_page(Cursor::decode(token)?, |source, token| async move {
        match source {
            Source::YouTubeMusic => api.library_more(section, token).await,
            Source::Spotify => crate::spotify::library_more(section, &token).await,
        }
    })
    .await
}

pub(crate) fn restart_spotify_song_paging(songs: &mut Paged<Track>) -> bool {
    let Some(mut cursor) = songs
        .next
        .as_deref()
        .and_then(|token| Cursor::decode(token).ok())
    else {
        return false;
    };
    if cursor.spotify.is_none() {
        return false;
    }
    cursor.spotify = Some(moosik_spotify::library_start_cursor(
        moosik_spotify::LibrarySection::Songs,
    ));
    cursor.spotify_next = true;
    songs.next = cursor.encode();
    songs.loading = false;
    true
}

pub(crate) fn append_library_songs(songs: &mut Paged<Track>, page: Page) {
    let mut tracks: Vec<_> = page
        .items
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Song(track) | Entry::Video(track) => Some(track),
            _ => None,
        })
        .collect();
    if tracks.iter().any(Track::is_spotify) {
        let existing: HashSet<_> = songs
            .items
            .iter()
            .map(|track| track.video_id.as_str())
            .collect();
        tracks.retain(|track| !track.is_spotify() || !existing.contains(track.video_id.as_str()));
    }
    songs.extend(tracks, page.next);
}

async fn continued_page<F, Fut>(mut cursor: Cursor, mut fetch: F) -> Result<Page>
where
    F: FnMut(Source, String) -> Fut,
    Fut: Future<Output = Result<Page>>,
{
    let mut errors = Vec::new();
    for source in cursor.order() {
        let Some(token) = cursor.token(source).map(str::to_owned) else {
            continue;
        };
        match fetch(source, token).await {
            Ok(mut page) => match cursor.advance(source, page.next.clone()) {
                Ok(()) => {
                    page.next = cursor.encode();
                    return Ok(page);
                }
                Err(error) => errors.push(format!("{}: {error:#}", source.name())),
            },
            Err(error) => errors.push(format!("{}: {error:#}", source.name())),
        }
    }
    if errors.is_empty() {
        bail!("Library continuation is empty");
    }
    bail!("{}", errors.join("\n"))
}

pub async fn home(
    api: &Api,
    filter: Option<&str>,
    fetch: &HomeFetch,
    sources: HomeSources,
) -> Result<Loaded<HomeFeed>> {
    let (youtube, spotify) = tokio::join!(
        async {
            if sources.youtube {
                Some(api.home(filter, fetch).await)
            } else {
                None
            }
        },
        async {
            if sources.spotify && filter.is_none() {
                Some(crate::spotify::home().await)
            } else {
                None
            }
        },
    );
    merge_home(youtube, spotify)
}

fn merge_home(
    youtube: Option<Result<HomeFeed>>,
    spotify: Option<Result<HomeFeed>>,
) -> Result<Loaded<HomeFeed>> {
    if youtube.is_none() && spotify.is_none() {
        return Ok(Loaded {
            value: HomeFeed::default(),
            warnings: Vec::new(),
        });
    }
    let (mut youtube, spotify, warnings) = collect(youtube, spotify)?;
    youtube.shelves.extend(spotify.shelves);
    Ok(Loaded {
        value: youtube,
        warnings,
    })
}

pub async fn new_releases(api: &Api, youtube: bool, spotify: bool) -> Result<Loaded<Vec<Shelf>>> {
    let (youtube, spotify) = tokio::join!(
        async {
            if youtube {
                Some(api.new_releases().await)
            } else {
                None
            }
        },
        async {
            if spotify {
                Some(crate::spotify::new_releases().await)
            } else {
                None
            }
        },
    );
    let (youtube, spotify, warnings) = collect(youtube, spotify)?;
    Ok(Loaded {
        value: merge_release_shelves(youtube.into_iter().chain(spotify)),
        warnings,
    })
}

fn merge_release_shelves(shelves: impl IntoIterator<Item = Shelf>) -> Vec<Shelf> {
    let mut releases = Vec::new();
    let mut other = Vec::new();
    let mut ids = HashSet::new();
    for shelf in shelves {
        if shelf.items.is_empty() {
            continue;
        }
        if shelf
            .items
            .iter()
            .all(|entry| matches!(entry, Entry::Album(_)))
        {
            releases.extend(shelf.items.into_iter().filter(
                |entry| matches!(entry, Entry::Album(album) if ids.insert(album.id.clone())),
            ));
        } else {
            other.push(shelf);
        }
    }
    if !releases.is_empty() {
        other.insert(
            0,
            Shelf {
                title: "New albums & singles".to_owned(),
                strapline: None,
                items: releases,
                more_params: None,
                more_browse_id: None,
                expanded: false,
            },
        );
    }
    other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_initial_provider_can_recover_without_replacing_healthy_pages() {
        let youtube = || Library {
            songs: Paged::new(
                vec![song("youtube", "YouTube song")],
                Some("youtube-more".into()),
            ),
            ..Default::default()
        };
        let spotify = || Library {
            songs: Paged::new(
                vec![song("spotify:track:0000000000000000000001", "Spotify song")],
                Some("spotify-more".into()),
            ),
            ..Default::default()
        };
        for failed in [Provider::YouTubeMusic, Provider::Spotify] {
            let failure = || Err(anyhow::anyhow!("unavailable"));
            let mut initial = assemble_library(
                Some(if failed == Provider::YouTubeMusic {
                    failure()
                } else {
                    Ok(youtube())
                }),
                Some(if failed == Provider::Spotify {
                    failure()
                } else {
                    Ok(spotify())
                }),
                Vec::new(),
            )
            .unwrap();
            assert_eq!(
                initial.failed_provider,
                Some((failed, "unavailable".into()))
            );
            let healthy_track = initial.value.songs[0].clone();
            let recovered = if failed == Provider::YouTubeMusic {
                youtube()
            } else {
                spotify()
            };
            merge_recovered_library(&mut initial.value, recovered, failed);
            assert_eq!(initial.value.songs.len(), 2);
            assert_eq!(initial.value.songs[0], healthy_track);
            let cursor = Cursor::decode(initial.value.songs.next.as_deref().unwrap()).unwrap();
            assert_eq!(cursor.youtube.as_deref(), Some("youtube-more"));
            assert_eq!(cursor.spotify.as_deref(), Some("spotify-more"));
            assert_eq!(cursor.spotify_next, failed == Provider::Spotify);
        }
    }

    #[test]
    fn new_releases_keep_both_album_sources_and_a_separate_video_shelf() {
        fn album(id: &str, title: &str) -> Entry {
            Entry::Album(crate::model::AlbumRef {
                id: id.to_owned(),
                title: title.to_owned(),
                artist: "Artist".to_owned(),
                artists: vec![crate::model::ArtistLink::plain("Artist")],
                explicit: None,
                year: Some("2026".to_owned()),
                kind: Some("Album".to_owned()),
                thumbnail: None,
            })
        }
        let mut youtube = home_feed("New albums & singles").shelves.remove(0);
        let yt_match = album("MPREbNew", "Matched release");
        youtube.items = vec![
            yt_match.clone(),
            yt_match,
            album("MPREbOnly", "YouTube only"),
        ];
        let mut spotify = home_feed("New albums & singles").shelves.remove(0);
        spotify.items = vec![
            album("spotify:album:0000000000000000000001", "Matched release"),
            album("spotify:album:0000000000000000000002", "Spotify only"),
        ];
        let mut videos = home_feed("New music videos").shelves.remove(0);
        videos.items.push(Entry::Video(song("video", "New video")));

        let mut shelves = merge_release_shelves([youtube, videos.clone(), spotify]);
        assert_eq!(shelves.len(), 2);
        assert_eq!(shelves[0].title, "New albums & singles");
        assert_eq!(shelves[0].items.len(), 4);
        assert_eq!(shelves[1], videos);
        let preferred = crate::model::Provider::Spotify;
        crate::source_groups::prioritize_entries(&mut shelves[0].items, Some(preferred));
        let groups = crate::source_groups::group_entries(&shelves[0].items, true);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].variants.len(), 2);
        assert_eq!(
            groups[1].preferred_entry(preferred, None).unwrap().title(),
            "Spotify only"
        );
        assert_eq!(
            groups[2].preferred_entry(preferred, None).unwrap().title(),
            "YouTube only"
        );
        let preferred = crate::model::Provider::YouTubeMusic;
        crate::source_groups::prioritize_entries(&mut shelves[0].items, Some(preferred));
        let groups = crate::source_groups::group_entries(&shelves[0].items, true);
        assert_eq!(
            groups[0].preferred_entry(preferred, None).unwrap().title(),
            "Matched release"
        );
        assert_eq!(
            groups[1].preferred_entry(preferred, None).unwrap().title(),
            "YouTube only"
        );
    }

    fn home_feed(source: &str) -> HomeFeed {
        HomeFeed {
            shelves: vec![crate::model::Shelf {
                title: source.to_owned(),
                strapline: None,
                items: Vec::new(),
                more_params: None,
                more_browse_id: None,
                expanded: false,
            }],
            chips: vec![crate::model::HomeChip {
                title: source.to_owned(),
                params: format!("{source}-filter"),
                selected: false,
            }],
            next: Some(format!("{source}-next")),
        }
    }

    #[test]
    fn home_with_no_remote_feeds_is_empty_without_a_sign_in_error() {
        let loaded = merge_home(None, None).unwrap();
        assert!(loaded.value.shelves.is_empty());
        assert!(loaded.value.chips.is_empty());
        assert!(loaded.value.next.is_none());
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn combined_home_keeps_youtube_controls_and_both_native_shelf_orders() {
        let loaded = merge_home(
            Some(Ok(home_feed("youtube"))),
            Some(Ok(home_feed("spotify"))),
        )
        .unwrap();
        let titles: Vec<_> = loaded
            .value
            .shelves
            .iter()
            .map(|shelf| shelf.title.as_str())
            .collect();
        assert_eq!(titles, ["youtube", "spotify"]);
        assert_eq!(loaded.value.chips.len(), 1);
        assert_eq!(loaded.value.chips[0].params, "youtube-filter");
        assert_eq!(loaded.value.next.as_deref(), Some("youtube-next"));
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn home_partial_failure_preserves_only_healthy_feed_data_and_warnings() {
        let loaded = merge_home(
            Some(Err(anyhow::anyhow!("unavailable"))),
            Some(Ok(home_feed("spotify"))),
        )
        .unwrap();
        assert_eq!(loaded.value.shelves[0].title, "spotify");
        assert!(loaded.value.chips.is_empty());
        assert!(loaded.value.next.is_none());
        assert_eq!(loaded.warnings, ["YouTube Music: unavailable"]);
        let loaded = merge_home(
            Some(Ok(home_feed("youtube"))),
            Some(Err(anyhow::anyhow!("expired"))),
        )
        .unwrap();
        assert_eq!(loaded.value.next.as_deref(), Some("youtube-next"));
        assert_eq!(loaded.warnings, ["Spotify: expired"]);
        let spotify_only = merge_home(None, Some(Ok(home_feed("spotify")))).unwrap();
        assert_eq!(spotify_only.value.shelves.len(), 1);
        assert!(spotify_only.value.chips.is_empty());
        assert!(spotify_only.value.next.is_none());
        assert!(
            merge_home(
                Some(Err(anyhow::anyhow!("unavailable"))),
                Some(Err(anyhow::anyhow!("expired")))
            )
            .is_err()
        );
    }

    fn song(id: &str, title: &str) -> Track {
        serde_json::from_value::<crate::playback::PersistedTrack>(
            serde_json::json!({"video_id": id, "title": title}),
        )
        .unwrap()
        .into_track()
    }

    #[tokio::test]
    async fn restarting_spotify_songs_recovers_the_shifted_page_boundary() {
        let spotify_song = |index| song(&format!("spotify:track:{index:022}"), "Saved song");
        let mut songs = merge_page(
            Paged::new(
                vec![song("youtube", "YouTube song")],
                Some("yt-next".into()),
            ),
            Paged::new(
                (1..50).map(spotify_song).collect(),
                Some("offset-50".into()),
            ),
        );
        songs.loading = true;

        assert!(restart_spotify_song_paging(&mut songs));
        assert!(!songs.loading);
        let cursor = Cursor::decode(songs.next.as_deref().unwrap()).unwrap();
        let page = continued_page(cursor, |source, token| {
            assert_eq!(source, Source::Spotify);
            assert_eq!(
                token,
                moosik_spotify::library_start_cursor(moosik_spotify::LibrarySection::Songs)
            );
            std::future::ready(Ok(Page {
                items: (1..=50).map(spotify_song).map(Entry::Song).collect(),
                next: None,
            }))
        })
        .await
        .unwrap();
        append_library_songs(&mut songs, page);

        assert_eq!(songs.items.len(), 51);
        assert_eq!(songs.items[0].video_id, "youtube");
        assert_eq!(
            songs.items.last().unwrap().video_id,
            spotify_song(50).video_id
        );
        let cursor = Cursor::decode(songs.next.as_deref().unwrap()).unwrap();
        assert_eq!(cursor.youtube.as_deref(), Some("yt-next"));
        assert!(cursor.spotify.is_none());
        assert!(!restart_spotify_song_paging(&mut songs));
    }

    #[tokio::test]
    async fn failed_provider_keeps_its_cursor_while_healthy_pages_advance() {
        for failed in [Source::YouTubeMusic, Source::Spotify] {
            let cursor = Cursor {
                youtube: Some("source-token".into()),
                spotify: Some("source-token".into()),
                spotify_next: failed == Source::Spotify,
            };
            let healthy = if failed == Source::YouTubeMusic {
                Source::Spotify
            } else {
                Source::YouTubeMusic
            };
            let mut calls = Vec::new();
            let page = continued_page(cursor, |source, token| {
                calls.push((source, token));
                std::future::ready(if source == failed {
                    Err(anyhow::anyhow!("session expired"))
                } else {
                    Ok(Page {
                        items: Vec::new(),
                        next: Some("healthy-next".into()),
                    })
                })
            })
            .await
            .unwrap();
            let cursor = Cursor::decode(page.next.as_deref().unwrap()).unwrap();
            assert_eq!(cursor.token(failed), Some("source-token"));
            assert_eq!(cursor.token(healthy), Some("healthy-next"));
            let page = continued_page(cursor, |source, token| {
                calls.push((source, token));
                std::future::ready(if source == failed {
                    Err(anyhow::anyhow!("still expired"))
                } else {
                    Ok(Page {
                        items: Vec::new(),
                        next: None,
                    })
                })
            })
            .await
            .unwrap();
            assert_eq!(
                calls,
                [
                    (failed, "source-token".into()),
                    (healthy, "source-token".into()),
                    (failed, "source-token".into()),
                    (healthy, "healthy-next".into())
                ]
            );
            let cursor = Cursor::decode(page.next.as_deref().unwrap()).unwrap();
            assert_eq!(cursor.token(failed), Some("source-token"));
            assert!(cursor.token(healthy).is_none());
        }
    }

    #[tokio::test]
    async fn continuation_errors_include_both_provider_failures() {
        let cursor = Cursor {
            youtube: Some("yt-token".into()),
            spotify: Some("sp-token".into()),
            spotify_next: true,
        };
        let error = continued_page(cursor, |source, _| {
            std::future::ready(Err(anyhow::anyhow!(match source {
                Source::YouTubeMusic => "session expired",
                Source::Spotify => "connection failed",
            })))
        })
        .await
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("YouTube Music: session expired"));
        assert!(error.contains("Spotify: connection failed"));
        assert!(!error.contains("yt-token"));
        assert!(!error.contains("sp-token"));
    }

    #[tokio::test]
    async fn nonadvancing_provider_falls_back_without_losing_its_token() {
        let cursor = Cursor {
            youtube: Some("yt-token".into()),
            spotify: Some("sp-token".into()),
            spotify_next: false,
        };
        let page = continued_page(cursor, |source, token| {
            std::future::ready(Ok(Page {
                items: Vec::new(),
                next: (source == Source::YouTubeMusic).then_some(token),
            }))
        })
        .await
        .unwrap();
        let cursor = Cursor::decode(page.next.as_deref().unwrap()).unwrap();
        assert_eq!(cursor.youtube.as_deref(), Some("yt-token"));
        assert!(cursor.spotify.is_none());
    }

    #[tokio::test]
    async fn merged_name_sort_reads_both_provider_tails_before_sorting() {
        for sort in [LibrarySortParam::NameAsc, LibrarySortParam::NameDesc] {
            let songs = merge_page(
                Paged::new(vec![song("yt-z", "Zulu")], Some("yt-next".into())),
                Paged::new(
                    vec![song("spotify:track:0000000000000000000001", "beta")],
                    Some("sp-next".into()),
                ),
            );
            let songs = sorted_songs(songs, sort, |token| async move {
                continued_page(Cursor::decode(&token)?, |source, token| {
                    let track = match source {
                        Source::YouTubeMusic => {
                            assert_eq!(token, "yt-next");
                            song("yt-a", "alpha")
                        }
                        Source::Spotify => {
                            assert_eq!(token, "sp-next");
                            song("spotify:track:0000000000000000000002", "Delta")
                        }
                    };
                    std::future::ready(Ok(Page {
                        items: vec![crate::model::Entry::Song(track)],
                        next: None,
                    }))
                })
                .await
            })
            .await
            .unwrap();
            let expected = if sort == LibrarySortParam::NameAsc {
                ["alpha", "beta", "Delta", "Zulu"]
            } else {
                ["Zulu", "Delta", "beta", "alpha"]
            };
            assert_eq!(
                songs
                    .items
                    .iter()
                    .map(|track| track.title.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(songs.next.is_none());
            assert!(!songs.loading);
        }
    }

    #[tokio::test]
    async fn name_sort_rejects_paging_cycles_and_partial_fetches() {
        let songs = || Paged::new(vec![song("yt-z", "Zulu")], Some("first".into()));
        let mut calls = 0;
        let result = sorted_songs(songs(), LibrarySortParam::NameAsc, |token| {
            calls += 1;
            std::future::ready(Ok(Page {
                items: vec![crate::model::Entry::Song(song("yt-a", "alpha"))],
                next: Some(if token == "first" { "second" } else { "first" }.into()),
            }))
        })
        .await;
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("continuation repeated")
        );
        assert_eq!(calls, 2);
        let result = sorted_songs(songs(), LibrarySortParam::NameDesc, |_| {
            std::future::ready(Err(anyhow::anyhow!("provider unavailable")))
        })
        .await;
        let error = format!("{:#}", result.err().unwrap());
        assert!(error.contains("complete library"));
        assert!(error.contains("provider unavailable"));
    }

    #[test]
    fn merged_pages_keep_both_continuations() {
        let page = merge_page(
            Paged::new(vec![1, 2], Some("yt-token".into())),
            Paged::new(vec![3], Some("sp-token".into())),
        );
        assert_eq!(page.items, [1, 2, 3]);
        let cursor = Cursor::decode(page.next.as_deref().unwrap()).unwrap();
        assert_eq!(cursor.youtube.as_deref(), Some("yt-token"));
        assert_eq!(cursor.spotify.as_deref(), Some("sp-token"));
        assert!(!cursor.spotify_next);
    }

    #[test]
    fn provider_failure_preserves_other_provider() {
        let (youtube, spotify, warnings) = collect(
            Some(Err(anyhow::anyhow!("session expired"))),
            Some(Ok(vec![1])),
        )
        .unwrap();
        assert!(youtube.is_empty());
        assert_eq!(spotify, [1]);
        assert_eq!(warnings, ["YouTube Music: session expired"]);
        assert!(collect::<Vec<u8>>(None, Some(Err(anyhow::anyhow!("expired")))).is_err());
    }

    #[test]
    fn completed_pages_have_no_continuation() {
        assert!(
            merge_page(Paged::new(vec![1], None), Paged::new(vec![2], None))
                .next
                .is_none()
        );
        assert!(Cursor::decode("providers:invalid").is_err());
        assert_eq!(
            Cursor::decode("old-token").unwrap().youtube.as_deref(),
            Some("old-token")
        );
    }
}
