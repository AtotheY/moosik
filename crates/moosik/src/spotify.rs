use std::{
    path::PathBuf,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use moosik_spotify::{
    CatalogItem, CatalogTrack, PlaylistSummary, TrackMetadata, Transport, WebSession,
};
use tokio::sync::Mutex;

use crate::model::{
    AccountProfile, Album, AlbumRef, Artist, ArtistActions, ArtistLink, ArtistRef, ChartTrend,
    Entry, HomeFeed, Library, LikeStatus, Paged, Playlist, PlaylistRef, PlaylistSource,
    SearchResults, SearchScope, Shelf, Track,
};

struct Cached {
    transport: Transport,
    expires_at: u64,
}

static TRANSPORT: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();
static SIGN_IN: Mutex<()> = Mutex::const_new(());
static SIGN_IN_STATUS: std::sync::Mutex<Option<(Instant, bool)>> = std::sync::Mutex::new(None);

fn session_path() -> Result<PathBuf> {
    crate::profile_dirs()
        .map(|dirs| dirs.config_dir().join("spotify-web.json"))
        .context("Could not find Spotify session storage")
}

pub fn is_signed_in() -> bool {
    let mut status = SIGN_IN_STATUS.lock().unwrap();
    if let Some((checked_at, signed_in)) = *status
        && checked_at.elapsed() < Duration::from_secs(1)
    {
        return signed_in;
    }
    let signed_in = session_path().is_ok_and(|path| path.is_file());
    *status = Some((Instant::now(), signed_in));
    signed_in
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn save(session: &WebSession) -> Result<()> {
    let path = session_path()?;
    std::fs::create_dir_all(
        path.parent()
            .context("Spotify session path has no parent")?,
    )?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = path.with_extension(format!("{}.{nonce}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options
        .open(&temporary)
        .context("Could not create Spotify session file")?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(session)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &path).context("Could not save Spotify session")
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    } else {
        *SIGN_IN_STATUS.lock().unwrap() = Some((Instant::now(), true));
    }
    result
}

pub async fn sign_in() -> Result<()> {
    let _guard = SIGN_IN
        .try_lock()
        .map_err(|_| anyhow::anyhow!("Spotify sign-in is already open"))?;
    let cookie = crate::spotify_web_login::capture().await?;
    let mut cached = TRANSPORT.get_or_init(|| Mutex::new(None)).lock().await;
    let previous = std::fs::read(session_path()?)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<WebSession>(&bytes).ok());
    let session = moosik_spotify::web_session(&cookie, previous.as_ref()).await?;
    save(&session)?;
    *cached = None;
    Ok(())
}

pub async fn sign_out() -> Result<()> {
    let _guard = SIGN_IN
        .try_lock()
        .map_err(|_| anyhow::anyhow!("Finish Spotify sign-in before signing out"))?;
    let mut cached = TRANSPORT.get_or_init(|| Mutex::new(None)).lock().await;
    crate::spotify_web_login::clear_profile().await?;
    match std::fs::remove_file(session_path()?) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => bail!("Could not remove the Spotify session"),
    }
    *SIGN_IN_STATUS.lock().unwrap() = Some((Instant::now(), false));
    *cached = None;
    Ok(())
}

pub(crate) async fn transport() -> Result<Transport> {
    let mut cached = TRANSPORT.get_or_init(|| Mutex::new(None)).lock().await;
    if let Some(entry) = cached
        .as_ref()
        .filter(|entry| entry.expires_at > now_ms().saturating_add(60_000))
    {
        return Ok(entry.transport.clone());
    }
    let bytes = match std::fs::read(session_path()?) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!("Sign in to Spotify in Settings first")
        }
        Err(_) => bail!("Could not read Spotify session"),
    };
    let previous: WebSession = serde_json::from_slice(&bytes)
        .context("Spotify session is invalid. Sign in again in Settings")?;
    let session = moosik_spotify::web_session(&previous.cookie, Some(&previous)).await?;
    let expires_at = session.expires_at.min(session.client_token_expires_at);
    save(&session)?;
    let transport = Transport::build_from_web_session(session).await?;
    *cached = Some(Cached {
        transport: transport.clone(),
        expires_at,
    });
    Ok(transport)
}

pub fn track_uri(input: &str) -> Option<String> {
    resource_uri(input).filter(|uri| uri.starts_with("spotify:track:"))
}

pub fn resource_uri(input: &str) -> Option<String> {
    moosik_spotify::resource_uri(input)
}

pub async fn account_profile() -> Result<AccountProfile> {
    let profile = transport().await?.account_profile().await?;
    Ok(AccountProfile {
        provider: crate::model::Provider::Spotify,
        id: profile.id,
        name: profile.name,
        thumbnail: profile.thumbnail.map(Into::into),
        followers: profile.followers,
        url: profile.url,
    })
}

pub async fn metadata(input: &str) -> Result<Option<Track>> {
    let Some(uri) = track_uri(input) else {
        return Ok(None);
    };
    let metadata = transport().await?.metadata(&uri).await?;
    Ok(Some(track(metadata)))
}

pub(crate) async fn autoplay_next(seed: &str, recent: &[String]) -> Result<Option<Track>> {
    Ok(transport()
        .await?
        .autoplay_next(seed, recent)
        .await?
        .map(catalog_track))
}

pub async fn search(query: &str) -> Result<SearchResults> {
    let items = search_scope(query, SearchScope::All).await?;
    Ok(SearchResults {
        no_results: items.is_empty(),
        items,
        ..Default::default()
    })
}

pub async fn search_scope(query: &str, scope: SearchScope) -> Result<Vec<Entry>> {
    use moosik_spotify::SearchKind;
    let kind = match scope {
        SearchScope::All => SearchKind::All,
        SearchScope::Songs => SearchKind::Tracks,
        SearchScope::Artists => SearchKind::Artists,
        SearchScope::Albums => SearchKind::Albums,
        SearchScope::Playlists => SearchKind::Playlists,
        SearchScope::Videos => return Ok(Vec::new()),
    };
    Ok(transport()
        .await?
        .search_catalog(query, kind)
        .await?
        .into_iter()
        .map(entry)
        .collect())
}

pub async fn home() -> Result<HomeFeed> {
    Ok(HomeFeed {
        shelves: transport()
            .await?
            .home()
            .await?
            .into_iter()
            .map(shelf)
            .collect(),
        ..Default::default()
    })
}

pub async fn new_releases() -> Result<Vec<Shelf>> {
    let items: Vec<_> = transport()
        .await?
        .search_catalog("tag:new", moosik_spotify::SearchKind::Albums)
        .await?
        .into_iter()
        .filter(|item| matches!(item, CatalogItem::Album(_)))
        .collect();
    if items.is_empty() {
        bail!("Spotify returned an empty New releases feed");
    }
    Ok(vec![shelf(moosik_spotify::Section {
        title: "New albums & singles".to_owned(),
        items,
    })])
}

pub async fn playlist(id: &str) -> Result<Playlist> {
    let playlist = transport().await?.playlist(id).await?;
    let summary = playlist.summary;
    Ok(Playlist {
        id: summary.uri,
        title: summary.title,
        description: playlist.description,
        author: summary.author,
        author_id: None,
        year: None,
        duration_text: None,
        track_count: summary.total.map(track_count),
        thumbnail: summary.image.map(Into::into),
        editable: false,
        in_library: playlist.saved,
        library_add: None,
        library_remove: None,
        saveable: false,
        tracks: Paged::new(
            playlist
                .tracks
                .items
                .into_iter()
                .map(catalog_track)
                .collect(),
            playlist.tracks.next,
        ),
        source: PlaylistSource::Spotify,
        extras: None,
    })
}

pub async fn playlist_more(token: &str) -> Result<crate::innertube::Page> {
    let page = transport().await?.playlist_more(token).await?;
    Ok(crate::innertube::Page {
        items: page
            .items
            .into_iter()
            .map(catalog_track)
            .map(Entry::Song)
            .collect(),
        next: page.next,
    })
}

pub async fn set_track_liked(uri: &str, liked: bool) -> Result<()> {
    Ok(transport().await?.set_track_liked(uri, liked).await?)
}

pub async fn remove_playlist_item(playlist_id: &str, uid: &str) -> Result<()> {
    Ok(transport()
        .await?
        .remove_playlist_item(playlist_id, uid)
        .await?)
}

pub async fn add_playlist_item(playlist_id: &str, track_uri: &str) -> Result<()> {
    Ok(transport()
        .await?
        .add_playlist_item(playlist_id, track_uri)
        .await?)
}

pub async fn editable_playlists(track_uri: &str) -> Result<crate::model::EditablePlaylists> {
    let (liked, playlists) = transport().await?.editable_playlists(track_uri).await?;
    Ok(crate::model::EditablePlaylists {
        liked,
        playlists: playlists
            .into_iter()
            .map(|(playlist, curates)| (playlist_ref(playlist, true), curates))
            .collect(),
    })
}

pub async fn album(id: &str) -> Result<Album> {
    let album = transport().await?.album(id).await?;
    let summary = album.summary;
    let artists = artist_links(summary.artists);
    Ok(Album {
        id: summary.uri,
        title: summary.title,
        artist: artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        artist_id: artists.iter().find_map(|artist| artist.id.clone()),
        artists,
        year: summary.year,
        kind: summary.kind.unwrap_or_else(|| "Album".to_owned()),
        description: None,
        thumbnail: summary.image.map(Into::into),
        duration_text: None,
        in_library: album.saved,
        like_id: None,
        tracks: album.tracks.into_iter().map(catalog_track).collect(),
    })
}

pub async fn artist(id: &str) -> Result<Artist> {
    let artist = transport().await?.artist(id).await?;
    let summary = artist.summary;
    Ok(Artist {
        id: summary.uri,
        name: summary.name,
        verified: artist.verified,
        official_artist_channel: false,
        subscribers: None,
        subscribers_long: None,
        followers: summary
            .followers
            .map(|count| format!("{} followers", format_count(count))),
        monthly_listeners: artist
            .monthly_listeners
            .map(|count| format!("{} monthly listeners", format_count(count))),
        views: None,
        actions: ArtistActions::default(),
        description: artist.description,
        thumbnail: summary.image.map(Into::into),
        hero: artist.hero.map(Into::into),
        top_songs: artist.top_tracks.into_iter().map(catalog_track).collect(),
        top_songs_title: Some("Popular".to_owned()),
        top_songs_playlist: None,
        shelves: artist
            .sections
            .into_iter()
            .map(shelf)
            .collect::<Vec<_>>()
            .into(),
    })
}

pub async fn library() -> Result<Library> {
    Ok(library_with_warnings().await?.0)
}

pub async fn library_with_warnings() -> Result<(Library, Vec<String>)> {
    use moosik_spotify::LibrarySection;
    let transport = transport().await?;
    let (songs, artists, albums, playlists) = tokio::join!(
        transport.library_page(LibrarySection::Songs, None),
        transport.library_page(LibrarySection::Artists, None),
        transport.library_page(LibrarySection::Albums, None),
        transport.library_page(LibrarySection::Playlists, None),
    );
    assemble_library(songs, artists, albums, playlists)
}

fn assemble_library(
    songs: moosik_spotify::Result<moosik_spotify::Page<CatalogItem>>,
    artists: moosik_spotify::Result<moosik_spotify::Page<CatalogItem>>,
    albums: moosik_spotify::Result<moosik_spotify::Page<CatalogItem>>,
    playlists: moosik_spotify::Result<moosik_spotify::Page<CatalogItem>>,
) -> Result<(Library, Vec<String>)> {
    use moosik_spotify::LibrarySection;
    let mut warnings = Vec::new();
    let mut take = |result, section, name| match result {
        Ok(page) => page,
        Err(error) => {
            warnings.push(format!("Spotify {name}: {error}"));
            moosik_spotify::Page {
                items: Vec::new(),
                next: Some(moosik_spotify::library_start_cursor(section)),
            }
        }
    };
    let songs = take(songs, LibrarySection::Songs, "saved songs");
    let artists = take(artists, LibrarySection::Artists, "followed artists");
    let albums = take(albums, LibrarySection::Albums, "albums");
    let playlists = take(playlists, LibrarySection::Playlists, "playlists");
    if warnings.len() == 4 {
        bail!("{}", warnings.join("\n"));
    }
    Ok((
        Library {
            songs: Paged::new(
                songs
                    .items
                    .into_iter()
                    .filter_map(|item| match entry(item) {
                        Entry::Song(track) => Some(track),
                        _ => None,
                    })
                    .collect(),
                songs.next,
            ),
            artists: Paged::new(
                artists
                    .items
                    .into_iter()
                    .filter_map(|item| match entry(item) {
                        Entry::Artist(artist) => Some(artist),
                        _ => None,
                    })
                    .collect(),
                artists.next,
            ),
            albums: Paged::new(
                albums
                    .items
                    .into_iter()
                    .filter_map(|item| match entry(item) {
                        Entry::Album(album) => Some(album),
                        _ => None,
                    })
                    .collect(),
                albums.next,
            ),
            playlists: Paged::new(
                playlists
                    .items
                    .into_iter()
                    .filter_map(|item| match entry(item) {
                        Entry::Playlist(playlist) => Some(playlist),
                        _ => None,
                    })
                    .collect(),
                playlists.next,
            ),
        },
        warnings,
    ))
}

pub async fn library_more(
    section: crate::api::LibrarySection,
    token: &str,
) -> Result<crate::innertube::Page> {
    let section = match section {
        crate::api::LibrarySection::Songs => moosik_spotify::LibrarySection::Songs,
        crate::api::LibrarySection::Artists => moosik_spotify::LibrarySection::Artists,
        crate::api::LibrarySection::Albums => moosik_spotify::LibrarySection::Albums,
        crate::api::LibrarySection::Playlists => moosik_spotify::LibrarySection::Playlists,
    };
    let page = transport()
        .await?
        .library_page(section, Some(token))
        .await?;
    Ok(crate::innertube::Page {
        items: page.items.into_iter().map(entry).collect(),
        next: page.next,
    })
}

pub async fn sidebar_playlists() -> Result<crate::innertube::Page> {
    let page = transport()
        .await?
        .library_page(moosik_spotify::LibrarySection::Playlists, None)
        .await?;
    Ok(crate::innertube::Page {
        items: page.items.into_iter().map(entry).collect(),
        next: page.next,
    })
}

pub(crate) fn format_count(count: u64) -> String {
    let suffixes = ["", "K", "M", "B", "T"];
    let mut value = count as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < suffixes.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        return count.to_string();
    }
    value = (value * 10.0).round() / 10.0;
    if value >= 1000.0 && unit + 1 < suffixes.len() {
        value /= 1000.0;
        unit += 1;
    }
    let number = format!("{value:.1}");
    format!(
        "{}{}",
        number.strip_suffix(".0").unwrap_or(&number),
        suffixes[unit]
    )
}

fn track_count(count: u64) -> String {
    format!(
        "{} {}",
        format_count(count),
        if count == 1 { "song" } else { "songs" }
    )
}

fn artist_links(artists: Vec<moosik_spotify::ArtistCredit>) -> Vec<ArtistLink> {
    artists
        .into_iter()
        .map(|artist| ArtistLink {
            name: artist.name,
            id: artist.uri,
        })
        .collect()
}

fn catalog_track(item: CatalogTrack) -> Track {
    let mut track = track(item.metadata);
    track.artists = artist_links(item.artists);
    track.album_id = item.album_uri;
    track.set_video_id = item.playlist_item_uid.map(Into::into);
    track.in_library = item.saved;
    track.like = if item.saved {
        LikeStatus::Like
    } else {
        LikeStatus::Indifferent
    };
    track.plays = item.plays.map(|plays| {
        let count = plays
            .trim()
            .parse::<u64>()
            .map(format_count)
            .unwrap_or(plays);
        format!("{count} plays")
    });
    track
}

fn entry(item: CatalogItem) -> Entry {
    match item {
        CatalogItem::Track(track) => Entry::Song(catalog_track(track)),
        CatalogItem::Artist(artist) => Entry::Artist(ArtistRef {
            id: artist.uri,
            name: artist.name,
            subscribers: artist
                .followers
                .map(|count| format!("{} followers", format_count(count))),
            thumbnail: artist.image.map(Into::into),
            actions: ArtistActions::default(),
        }),
        CatalogItem::Album(album) => Entry::Album(AlbumRef {
            id: album.uri,
            title: album.title,
            artist: album
                .artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            artists: artist_links(album.artists),
            explicit: None,
            year: album.year,
            kind: album.kind,
            thumbnail: album.image.map(Into::into),
        }),
        CatalogItem::Playlist(playlist) => Entry::Playlist(playlist_ref(playlist, false)),
    }
}

fn playlist_ref(playlist: PlaylistSummary, editable: bool) -> PlaylistRef {
    PlaylistRef {
        id: playlist.uri,
        title: playlist.title,
        author: playlist.author,
        author_id: None,
        track_count: playlist.total.map(track_count),
        thumbnail: playlist.image.map(Into::into),
        editable,
        source: PlaylistSource::Spotify,
    }
}

fn shelf(section: moosik_spotify::Section) -> Shelf {
    Shelf {
        title: section.title,
        strapline: None,
        items: section.items.into_iter().map(entry).collect(),
        more_params: None,
        more_browse_id: None,
        expanded: false,
    }
}

fn track(metadata: TrackMetadata) -> Track {
    Track {
        video_id: metadata.uri,
        title: metadata.title,
        artists: metadata
            .artist_names
            .into_iter()
            .map(ArtistLink::plain)
            .collect(),
        album: metadata.album,
        album_id: None,
        duration: metadata.duration_ms.map(|ms| ms / 1000),
        plays: None,
        explicit: metadata.explicit,
        like: LikeStatus::Indifferent,
        in_library: false,
        library_add: None,
        library_remove: None,
        set_video_id: None,
        thumbnail: metadata.cover_url.map(Into::into),
        wide: false,
        rank: None,
        trend: ChartTrend::default(),
        local_path: None,
    }
}

pub async fn play(
    uri: &str,
    handle: crate::spotify_drm::Handle,
    volume: f64,
    start_at: Option<f64>,
) -> Result<()> {
    let started = std::time::Instant::now();
    let result = match transport().await {
        Ok(transport) => {
            tracing::info!(
                elapsed_ms = started.elapsed().as_millis(),
                "Spotify transport ready"
            );
            crate::spotify_drm::play(transport, uri, handle.clone(), volume, start_at).await
        }
        Err(error) => Err(error),
    };
    if result.is_err() {
        handle.stop();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_entries_keep_artist_credits_and_unknown_explicitness() {
        let Entry::Album(album) = entry(CatalogItem::Album(moosik_spotify::AlbumSummary {
            uri: "spotify:album:album".to_owned(),
            title: "An album".to_owned(),
            artists: vec![
                moosik_spotify::ArtistCredit {
                    name: "Ann".to_owned(),
                    uri: Some("spotify:artist:ann".to_owned()),
                },
                moosik_spotify::ArtistCredit {
                    name: "Bo".to_owned(),
                    uri: None,
                },
            ],
            image: Some("https://example.test/album.jpg".to_owned()),
            year: Some("2024".to_owned()),
            kind: Some("Album".to_owned()),
        })) else {
            panic!("expected an album");
        };
        assert_eq!(album.id, "spotify:album:album");
        assert_eq!(album.title, "An album");
        assert_eq!(album.artist, "Ann, Bo");
        assert_eq!(
            album.artists,
            vec![
                crate::model::ArtistLink {
                    name: "Ann".to_owned(),
                    id: Some("spotify:artist:ann".to_owned()),
                },
                crate::model::ArtistLink::plain("Bo"),
            ]
        );
        assert_eq!(album.explicit, None);
        assert_eq!(album.year.as_deref(), Some("2024"));
        assert_eq!(album.kind.as_deref(), Some("Album"));
        assert_eq!(
            album.thumbnail.as_deref(),
            Some("https://example.test/album.jpg")
        );
    }

    #[test]
    fn partial_library_keeps_loaded_sections_and_failed_section_retry() {
        let (library, warnings) = assemble_library(
            Err(moosik_spotify::Error::Network("saved tracks")),
            Ok(moosik_spotify::Page {
                items: Vec::new(),
                next: Some("artists-next".to_owned()),
            }),
            Ok(moosik_spotify::Page {
                items: vec![CatalogItem::Album(moosik_spotify::AlbumSummary {
                    uri: "spotify:album:4aawyAB9vmqN3uQ7FjRGTy".to_owned(),
                    title: "An album".to_owned(),
                    artists: Vec::new(),
                    image: None,
                    year: None,
                    kind: None,
                })],
                next: None,
            }),
            Ok(moosik_spotify::Page {
                items: Vec::new(),
                next: None,
            }),
        )
        .unwrap();
        assert_eq!(library.albums.items.len(), 1);
        assert_eq!(library.artists.next.as_deref(), Some("artists-next"));
        assert_eq!(
            library.songs.next,
            Some(moosik_spotify::library_start_cursor(
                moosik_spotify::LibrarySection::Songs
            ))
        );
        assert!(library.playlists.next.is_none());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("saved songs"));
    }

    #[test]
    fn library_reports_failure_when_every_section_failed() {
        let failed = || Err(moosik_spotify::Error::Network("library"));
        let error = assemble_library(failed(), failed(), failed(), failed()).unwrap_err();
        let message = error.to_string();
        for section in ["saved songs", "followed artists", "albums", "playlists"] {
            assert!(message.contains(section));
        }
    }

    #[test]
    fn spotify_track_links_keep_their_source() {
        let uri = "spotify:track:3TO7bbrUKrOSPGRTB5MeCz";
        assert_eq!(track_uri(uri).as_deref(), Some(uri));
        assert_eq!(
            track_uri("https://open.spotify.com/track/3TO7bbrUKrOSPGRTB5MeCz?si=test").as_deref(),
            Some(uri)
        );
        assert_eq!(
            track_uri("https://open.spotify.com/intl-ro/track/3TO7bbrUKrOSPGRTB5MeCz").as_deref(),
            Some(uri)
        );
        assert!(
            track_uri("https://open.spotify.com.evil.test/track/3TO7bbrUKrOSPGRTB5MeCz").is_none()
        );
        assert!(track_uri("https://open.spotify.com/track/short").is_none());
        assert!(track_uri("https://open.spotify.com/album/3TO7bbrUKrOSPGRTB5MeCz").is_none());
    }
}
