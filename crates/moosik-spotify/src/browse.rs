use std::{
    collections::{HashSet, VecDeque},
    future::Future,
    time::Duration,
};

use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Error, Result, TrackMetadata, Transport, artwork::image};

const PAGE_SIZE: u64 = 50;
const PLAYLIST_HASH: &str = "21ea11a6ac75a5f3e01fd365d29c18e0a4a8b7dc09f89bbb5d73910190c6acaf";
const PLAYLIST_MUTATION_HASH: &str =
    "b907ad6b088d5bf7680fa81198996d58398c2d55dfcdf59fcc959e0be98a418a";
const EDITABLE_PLAYLISTS_HASH: &str =
    "d5c4b8096437dcc2ac9528c91dfcd299e35b747cda2f8f75d28f41f49c5092ba";
const APPLY_CURATIONS_HASH: &str =
    "05b739a3a73091c213385233b9d3ed8a857c2ca29d2eebadb3d04ed12e288697";
const ALBUM_HASH: &str = "0b6ce3511e1b02e4a40fa95440f992acbe85080eb308af9194322d708fc1914f";
const ARTIST_HASH: &str = "eafb1620d783ece118e72fbfede2b0064483e029d3c1aafafa1c2de0e25a89e7";
const LIBRARY_HASH: &str = "390c78e5b951029bad359785e69b07b536a509c581cbcd0aded5e5067f187455";
const SONGS_HASH: &str = "087278b20b743578a6262c2b0b4bcd20d879c503cc359a2285baf083ef944240";
const HOME_HASH: &str = "5fb7da7a03ee2776c0670856609cb85a189a3cecc36cc160b91cde79063b9cf1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistCredit {
    pub name: String,
    pub uri: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CatalogTrack {
    pub metadata: TrackMetadata,
    pub artists: Vec<ArtistCredit>,
    pub album_uri: Option<String>,
    pub saved: bool,
    pub plays: Option<String>,
    pub added_at: Option<String>,
    pub playlist_item_uid: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ArtistSummary {
    pub uri: String,
    pub name: String,
    pub image: Option<String>,
    pub followers: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct AlbumSummary {
    pub uri: String,
    pub title: String,
    pub artists: Vec<ArtistCredit>,
    pub image: Option<String>,
    pub year: Option<String>,
    pub kind: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PlaylistSummary {
    pub uri: String,
    pub title: String,
    pub author: String,
    pub image: Option<String>,
    pub total: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum CatalogItem {
    Track(CatalogTrack),
    Album(AlbumSummary),
    Artist(ArtistSummary),
    Playlist(PlaylistSummary),
}

#[derive(Clone, Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Section {
    pub title: String,
    pub items: Vec<CatalogItem>,
}

pub struct Playlist {
    pub summary: PlaylistSummary,
    pub description: Option<String>,
    pub saved: bool,
    pub tracks: Page<CatalogTrack>,
}

pub struct Album {
    pub summary: AlbumSummary,
    pub saved: bool,
    pub tracks: Vec<CatalogTrack>,
}

pub struct Artist {
    pub summary: ArtistSummary,
    pub verified: bool,
    pub description: Option<String>,
    pub monthly_listeners: Option<u64>,
    pub hero: Option<String>,
    pub top_tracks: Vec<CatalogTrack>,
    pub sections: Vec<Section>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LibrarySection {
    Songs,
    Playlists,
    Albums,
    Artists,
}

#[derive(Clone, Copy)]
pub enum SearchKind {
    All,
    Tracks,
    Albums,
    Artists,
    Playlists,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    section: Option<LibrarySection>,
    uri: Option<String>,
    offset: u64,
}

pub fn library_start_cursor(section: LibrarySection) -> String {
    encode_cursor(&Cursor {
        section: Some(section),
        uri: None,
        offset: 0,
    })
}

pub fn resource_uri(input: &str) -> Option<String> {
    let input = input.trim();
    if let Some(rest) = input.strip_prefix("spotify:") {
        let (kind, id) = rest.split_once(':')?;
        return valid_resource(kind, id).then(|| input.to_owned());
    }
    let url = url::Url::parse(input).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("open.spotify.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let mut parts = url.path_segments()?;
    let mut kind = parts.next()?;
    if kind.starts_with("intl-") {
        kind = parts.next()?;
    }
    let id = parts.next()?;
    if parts.any(|part| !part.is_empty()) || !valid_resource(kind, id) {
        return None;
    }
    Some(format!("spotify:{kind}:{id}"))
}

fn valid_resource(kind: &str, id: &str) -> bool {
    matches!(kind, "track" | "album" | "artist" | "playlist")
        && id.len() == 22
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn require_uri(input: &str, kind: &str) -> Result<String> {
    resource_uri(input)
        .filter(|uri| {
            uri.strip_prefix("spotify:")
                .is_some_and(|rest| rest.starts_with(&format!("{kind}:")))
        })
        .ok_or(Error::Invalid("resource URI"))
}

impl Transport {
    pub async fn search_catalog(&self, query: &str, kind: SearchKind) -> Result<Vec<CatalogItem>> {
        let query = query.trim();
        if query.is_empty() || query.len() > 1024 {
            return Err(Error::Invalid("search query"));
        }
        let (operation, hash, keys): (_, _, &[&str]) = match kind {
            SearchKind::All => (
                "searchDesktop",
                "eef7cc54888d91bdd6802623477873caa3948ae173a0c34fd86827b267e94c03",
                &[
                    "topResultsV2",
                    "tracksV2",
                    "artists",
                    "albumsV2",
                    "playlists",
                ],
            ),
            SearchKind::Tracks => (
                "searchTracks",
                "b02683192a98dde7966b5e6655a79eeb62713eab703eda9902c932818dd52751",
                &["tracksV2"],
            ),
            SearchKind::Artists => (
                "searchArtists",
                "7bf95d754fdbe32c8b161fbbe54d1ae50974900df4dce4c8f1afcbcad153224d",
                &["artists"],
            ),
            SearchKind::Albums => (
                "searchAlbums",
                "202cb3305e31e5a0767ba7925f28bd728cf8f8b0217e6da43909056071cd70e9",
                &["albumsV2"],
            ),
            SearchKind::Playlists => (
                "searchPlaylists",
                "d520014e748f9ea44f7707d8df1819867ac1205e8b7f3e28f22fe5fc858921b1",
                &["playlists"],
            ),
        };
        let response = self.query(operation, hash, json!({
            "searchTerm": query, "offset": 0, "limit": 30, "numberOfTopResults": 10,
            "includeArtistHasConcertsField": false, "includeLocalConcertsField": false,
            "includeAudiobooks": false, "includeAuthors": false, "includePreReleases": false,
            "includeAlbumPreReleases": false, "includeEpisodeContentRatingsV2": false
        })).await?;
        let items = parse_search(&response, keys)?;
        Ok(collapse_search_recordings(
            items,
            |uri| async move { self.canonical_track_uri(&uri).await },
        )
        .await)
    }

    pub async fn playlist(&self, input: &str) -> Result<Playlist> {
        let uri = require_uri(input, "playlist")?;
        let response = self.playlist_query(&uri, 0).await?;
        let data = response
            .pointer("/data/playlistV2")
            .ok_or(Error::Invalid("playlist"))?;
        let summary = playlist_summary(data, Some(&uri)).ok_or(Error::Invalid("playlist"))?;
        let tracks = parse_playlist_tracks(
            data,
            Cursor {
                section: None,
                uri: Some(uri),
                offset: 0,
            },
        )?;
        Ok(Playlist {
            summary,
            description: text(data, "/description").and_then(playlist_description),
            saved: saved(data),
            tracks,
        })
    }

    async fn playlist_query(&self, uri: &str, offset: u64) -> Result<Value> {
        self.query("fetchPlaylist", PLAYLIST_HASH, json!({
            "uri": uri, "offset": offset, "limit": PAGE_SIZE, "enableWatchFeedEntrypoint": false,
            "includeEpisodeContentRatingsV2": false
        })).await
    }

    pub async fn playlist_more(&self, token: &str) -> Result<Page<CatalogTrack>> {
        let cursor = decode_cursor(token)?;
        if cursor.section.is_some() {
            return Err(Error::Invalid("playlist page"));
        }
        let uri = require_uri(
            cursor
                .uri
                .as_deref()
                .ok_or(Error::Invalid("playlist page"))?,
            "playlist",
        )?;
        let response = self.playlist_query(&uri, cursor.offset).await?;
        let data = response
            .pointer("/data/playlistV2")
            .ok_or(Error::Invalid("playlist tracks"))?;
        parse_playlist_tracks(data, cursor)
    }

    pub async fn remove_playlist_item(&self, playlist_id: &str, uid: &str) -> Result<()> {
        let uri = require_uri(playlist_id, "playlist")?;
        if !valid_playlist_item_uid(uid) {
            return Err(Error::Invalid("playlist item ID"));
        }
        self.mutate(
            "removeFromPlaylist",
            PLAYLIST_MUTATION_HASH,
            json!({"playlistUri": uri, "uids": [uid]}),
        )
        .await
    }

    pub async fn add_playlist_item(&self, playlist_id: &str, track_uri: &str) -> Result<()> {
        let playlist_uri = require_uri(playlist_id, "playlist")?;
        let track_uri = require_uri(track_uri, "track")?;
        let response = self
            .query(
                "applyCurations",
                APPLY_CURATIONS_HASH,
                json!({
                    "input": {
                        "curations": [{"contextUri": playlist_uri, "curationType": "CURATE"}],
                        "itemUris": [track_uri]
                    }
                }),
            )
            .await?;
        let result = &response["data"]["applyCurations"][0];
        if result["__typename"].as_str() == Some("TrackResponseWrapper")
            && result["data"]["__typename"].as_str() == Some("Track")
        {
            Ok(())
        } else {
            Err(Error::Invalid("playlist addition"))
        }
    }

    pub async fn editable_playlists(
        &self,
        track_uri: &str,
    ) -> Result<(Option<bool>, Vec<(PlaylistSummary, bool)>)> {
        let track_uri = require_uri(track_uri, "track")?;
        let mut folders = VecDeque::from([None]);
        let mut seen = HashSet::new();
        let mut liked = None;
        let mut playlists = Vec::new();
        while let Some(folder) = folders.pop_front() {
            let mut offset = 0;
            loop {
                let response = self
                    .query(
                        "editablePlaylists",
                        EDITABLE_PLAYLISTS_HASH,
                        json!({
                            "uris": [track_uri],
                            "offset": offset,
                            "limit": PAGE_SIZE,
                            "textFilter": "",
                            "folderUri": folder
                        }),
                    )
                    .await?;
                let data = response
                    .pointer("/data/me/editablePlaylists")
                    .ok_or(Error::Invalid("editable playlists"))?;
                let page = parse_playlist_choices(data, offset)?;
                liked = liked.or(page.liked);
                folders.extend(
                    page.folders
                        .into_iter()
                        .filter(|uri| seen.insert(uri.clone()))
                        .map(Some),
                );
                playlists.extend(
                    page.playlists
                        .into_iter()
                        .filter(|(playlist, _)| seen.insert(playlist.uri.clone())),
                );
                let Some(next) = page.next else {
                    break;
                };
                offset = next;
            }
        }
        Ok((liked, playlists))
    }

    pub async fn album(&self, input: &str) -> Result<Album> {
        let uri = require_uri(input, "album")?;
        let response = self.album_query(&uri, 0).await?;
        let data = response
            .pointer("/data/albumUnion")
            .ok_or(Error::Invalid("album"))?;
        let summary = album_summary(data, Some(&uri)).ok_or(Error::Invalid("album"))?;
        let mut tracks = parse_album_tracks(data, &summary)?;
        let total = data["tracksV2"]["totalCount"]
            .as_u64()
            .ok_or(Error::Invalid("album tracks"))?;
        let mut offset = array(&data["tracksV2"]["items"]).len() as u64;
        while offset < total {
            let response = self.album_query(&uri, offset).await?;
            let data = response
                .pointer("/data/albumUnion")
                .ok_or(Error::Invalid("album"))?;
            let count = array(&data["tracksV2"]["items"]).len() as u64;
            if count == 0 || offset > 10_000 {
                return Err(Error::Invalid("album pagination"));
            }
            tracks.extend(parse_album_tracks(data, &summary)?);
            offset += count;
        }
        Ok(Album {
            summary,
            saved: saved(data),
            tracks,
        })
    }

    async fn album_query(&self, uri: &str, offset: u64) -> Result<Value> {
        self.query(
            "getAlbum",
            ALBUM_HASH,
            json!({"uri": uri, "locale": "", "offset": offset, "limit": PAGE_SIZE}),
        )
        .await
    }

    pub async fn artist(&self, input: &str) -> Result<Artist> {
        let uri = require_uri(input, "artist")?;
        let (response, verified) = tokio::join!(
            self.query("queryArtistOverview", ARTIST_HASH, json!({"uri": uri, "locale": "", "preReleaseV2": false, "includeMusicVideoPreviews": false})),
            self.artist_verified(&uri),
        );
        let response = response?;
        let data = response
            .pointer("/data/artistUnion")
            .ok_or(Error::Invalid("artist"))?;
        parse_artist(data, &uri, verified.unwrap_or(false))
    }

    pub async fn library_page(
        &self,
        section: LibrarySection,
        token: Option<&str>,
    ) -> Result<Page<CatalogItem>> {
        let cursor = match token {
            Some(token) => {
                let cursor = decode_cursor(token)?;
                if cursor.section != Some(section) || cursor.uri.is_some() {
                    return Err(Error::Invalid("library page"));
                }
                cursor
            }
            None => Cursor {
                section: Some(section),
                uri: None,
                offset: 0,
            },
        };
        if section == LibrarySection::Songs {
            let response = self
                .query(
                    "fetchLibraryTracks",
                    SONGS_HASH,
                    json!({"offset": cursor.offset, "limit": PAGE_SIZE}),
                )
                .await?;
            let data = response
                .pointer("/data/me/library/tracks")
                .ok_or(Error::Invalid("saved tracks"))?;
            let page = parse_track_page(data, cursor, TrackPageKind::SavedSongs)?;
            return Ok(Page {
                items: page.items.into_iter().map(CatalogItem::Track).collect(),
                next: page.next,
            });
        }
        let filter = match section {
            LibrarySection::Playlists => "Playlists",
            LibrarySection::Albums => "Albums",
            LibrarySection::Artists => "Artists",
            LibrarySection::Songs => unreachable!(),
        };
        let response = self.query("libraryV3", LIBRARY_HASH, json!({
            "filters": [filter], "order": null, "textFilter": "", "features": ["LIKED_SONGS", "YOUR_EPISODES_V2", "PRERELEASES", "EVENTS"],
            "limit": PAGE_SIZE, "offset": cursor.offset, "flatten": true, "expandedFolders": [], "folderUri": null, "includeFoldersWhenFlattening": false
        })).await?;
        let data = response
            .pointer("/data/me/libraryV3")
            .ok_or(Error::Invalid("library"))?;
        parse_library_page(data, cursor)
    }

    pub async fn home(&self) -> Result<Vec<Section>> {
        let response = self.query("home", HOME_HASH, json!({
            "homeEndUserIntegration": "INTEGRATION_WEB_PLAYER", "timeZone": "UTC", "sp_t": "", "facet": "", "sectionItemsLimit": 12,
            "includeEpisodeContentRatingsV2": false, "includeHomeChapterVideoCards": false
        })).await?;
        parse_home(&response)
    }
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn playlist_description(description: String) -> Option<String> {
    if !description.contains(['<', '&']) {
        return Some(description);
    }
    let input = html5ever::tokenizer::BufferQueue::default();
    input.push_back(description.as_str().into());
    let tokenizer = html5ever::tokenizer::Tokenizer::new(
        DescriptionText(std::cell::RefCell::new(String::new())),
        Default::default(),
    );
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let plain = tokenizer.sink.0.into_inner();
    let trimmed = plain.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

struct DescriptionText(std::cell::RefCell<String>);

impl html5ever::tokenizer::TokenSink for DescriptionText {
    type Handle = ();

    fn process_token(
        &self,
        token: html5ever::tokenizer::Token,
        _: u64,
    ) -> html5ever::tokenizer::TokenSinkResult<()> {
        use html5ever::tokenizer::{TagKind, Token, TokenSinkResult};

        match token {
            Token::CharacterTokens(text) => self.0.borrow_mut().push_str(&text),
            Token::TagToken(tag) if tag.kind == TagKind::StartTag && tag.name.as_ref() == "br" => {
                self.0.borrow_mut().push('\n');
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

fn text(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn saved(value: &Value) -> bool {
    value["saved"]
        .as_bool()
        .or_else(|| value["isSaved"].as_bool())
        .unwrap_or(false)
}

fn credits(value: &Value) -> Vec<ArtistCredit> {
    array(&value["items"])
        .iter()
        .filter_map(|artist| {
            Some(ArtistCredit {
                name: text(artist, "/profile/name").or_else(|| text(artist, "/name"))?,
                uri: text(artist, "/uri").and_then(|uri| require_uri(&uri, "artist").ok()),
            })
        })
        .collect()
}

fn unwrap_item(mut value: &Value) -> (&Value, Option<&str>) {
    let mut uri = None;
    for _ in 0..5 {
        uri = value["_uri"]
            .as_str()
            .or_else(|| value["uri"].as_str())
            .or(uri);
        if matches!(
            value["__typename"].as_str(),
            Some("Track" | "Album" | "Artist" | "Playlist")
        ) {
            break;
        }
        let next = ["item", "itemV2", "content", "track", "data"]
            .iter()
            .find_map(|key| value.get(key).filter(|value| value.is_object()));
        let Some(next) = next else {
            break;
        };
        value = next;
    }
    (value, uri)
}

fn catalog_item(value: &Value) -> Option<CatalogItem> {
    let (data, uri) = unwrap_item(value);
    let uri = uri.or_else(|| data["uri"].as_str())?;
    let kind = data["__typename"]
        .as_str()
        .unwrap_or_else(|| match uri.split(':').nth(1) {
            Some("track") => "Track",
            Some("album") => "Album",
            Some("artist") => "Artist",
            Some("playlist") => "Playlist",
            _ => "",
        });
    match kind {
        "Track" => parse_track(data, Some(uri), false).map(CatalogItem::Track),
        "Album" => album_summary(data, Some(uri)).map(CatalogItem::Album),
        "Artist" => artist_summary(data, Some(uri)).map(CatalogItem::Artist),
        "Playlist" => playlist_summary(data, Some(uri)).map(CatalogItem::Playlist),
        _ => None,
    }
}

fn item_uri(item: &CatalogItem) -> &str {
    match item {
        CatalogItem::Track(track) => &track.metadata.uri,
        CatalogItem::Album(album) => &album.uri,
        CatalogItem::Artist(artist) => &artist.uri,
        CatalogItem::Playlist(playlist) => &playlist.uri,
    }
}

fn parse_track(data: &Value, uri: Option<&str>, force_saved: bool) -> Option<CatalogTrack> {
    if data["playability"]["playable"].as_bool() == Some(false)
        || data["__typename"]
            .as_str()
            .is_some_and(|kind| kind != "Track")
    {
        return None;
    }
    let uri = require_uri(data["uri"].as_str().or(uri)?, "track").ok()?;
    let artists = credits(&data["artists"]);
    let album = &data["albumOfTrack"];
    Some(CatalogTrack {
        metadata: TrackMetadata {
            uri,
            title: text(data, "/name")?,
            artist_names: artists.iter().map(|artist| artist.name.clone()).collect(),
            album: text(album, "/name"),
            duration_ms: data["duration"]["totalMilliseconds"].as_u64(),
            cover_url: image(&album["coverArt"]["sources"]),
            explicit: data["contentRating"]["label"].as_str() == Some("EXPLICIT"),
        },
        artists,
        album_uri: text(album, "/uri").and_then(|uri| require_uri(&uri, "album").ok()),
        saved: force_saved || saved(data),
        plays: text(data, "/playcount")
            .or_else(|| data["playcount"].as_u64().map(|count| count.to_string())),
        playlist_item_uid: None,
        added_at: None,
    })
}

fn album_summary(data: &Value, uri: Option<&str>) -> Option<AlbumSummary> {
    Some(AlbumSummary {
        uri: require_uri(data["uri"].as_str().or(uri)?, "album").ok()?,
        title: text(data, "/name")?,
        artists: credits(&data["artists"]),
        image: image(&data["coverArt"]["sources"]),
        year: data["date"]["year"]
            .as_u64()
            .map(|year| year.to_string())
            .or_else(|| text(data, "/date/isoString").map(|date| date.chars().take(4).collect())),
        kind: text(data, "/type").map(|kind| match kind.as_str() {
            "ALBUM" => "Album".to_owned(),
            "SINGLE" => "Single".to_owned(),
            "EP" => "EP".to_owned(),
            "COMPILATION" => "Compilation".to_owned(),
            _ => kind,
        }),
    })
}

fn artist_summary(data: &Value, uri: Option<&str>) -> Option<ArtistSummary> {
    Some(ArtistSummary {
        uri: require_uri(data["uri"].as_str().or(uri)?, "artist").ok()?,
        name: text(data, "/profile/name").or_else(|| text(data, "/name"))?,
        image: image(&data["visuals"]["avatarImage"]["sources"]),
        followers: data["stats"]["followers"].as_u64(),
    })
}

fn playlist_summary(data: &Value, uri: Option<&str>) -> Option<PlaylistSummary> {
    Some(PlaylistSummary {
        uri: require_uri(data["uri"].as_str().or(uri)?, "playlist").ok()?,
        title: text(data, "/name")?,
        author: text(data, "/ownerV2/data/name").unwrap_or_default(),
        image: array(&data["images"]["items"])
            .iter()
            .find_map(|item| image(&item["sources"])),
        total: data["content"]["totalCount"].as_u64(),
    })
}

#[derive(Default)]
struct PlaylistChoicesPage {
    liked: Option<bool>,
    playlists: Vec<(PlaylistSummary, bool)>,
    folders: Vec<String>,
    next: Option<u64>,
}

fn parse_playlist_choices(data: &Value, offset: u64) -> Result<PlaylistChoicesPage> {
    let items = data["items"]
        .as_array()
        .ok_or(Error::Invalid("editable playlist items"))?;
    let total = data["totalCount"]
        .as_u64()
        .filter(|total| *total <= 1_000_000)
        .ok_or(Error::Invalid("editable playlist total"))?;
    let next = offset
        .checked_add(items.len() as u64)
        .ok_or(Error::Invalid("editable playlist offset"))?;
    if items.is_empty() && next < total {
        return Err(Error::Invalid("empty editable playlist page"));
    }
    let mut page = PlaylistChoicesPage {
        next: (next < total).then_some(next),
        ..Default::default()
    };
    for item in items {
        let data = &item["item"]["data"];
        match data["__typename"].as_str() {
            Some("Playlist") => {
                if let (Some(playlist), Some(curates)) =
                    (playlist_summary(data, None), item["curates"].as_bool())
                {
                    page.playlists.push((playlist, curates));
                }
            }
            Some("PseudoPlaylist") if data["uri"].as_str() == Some("spotify:collection:tracks") => {
                page.liked = item["curates"].as_bool();
            }
            Some("Folder") => {
                if let Some(uri) = data["uri"]
                    .as_str()
                    .filter(|uri| uri.starts_with("spotify:") && uri.len() <= 512)
                {
                    page.folders.push(uri.to_owned());
                }
            }
            _ => {}
        }
    }
    Ok(page)
}

fn next_cursor(data: &Value, mut cursor: Cursor) -> Result<Option<String>> {
    let items = data["items"]
        .as_array()
        .ok_or(Error::Invalid("page items"))?;
    let total = data["totalCount"]
        .as_u64()
        .ok_or(Error::Invalid("page total"))?;
    let received = items.len() as u64;
    if received == 0 && cursor.offset < total {
        return Err(Error::Invalid("empty page"));
    }
    cursor.offset = cursor
        .offset
        .checked_add(received)
        .ok_or(Error::Invalid("page offset"))?;
    if cursor.offset >= total {
        return Ok(None);
    }
    Ok(Some(encode_cursor(&cursor)))
}

fn encode_cursor(cursor: &Cursor) -> String {
    format!(
        "spotify-page:{}",
        serde_json::to_string(cursor).expect("page cursor serialization")
    )
}

fn decode_cursor(token: &str) -> Result<Cursor> {
    let token = token
        .strip_prefix("spotify-page:")
        .filter(|token| token.len() <= 512)
        .ok_or(Error::Invalid("page cursor"))?;
    let cursor: Cursor = serde_json::from_str(token).map_err(|_| Error::Invalid("page cursor"))?;
    if cursor.offset > 1_000_000
        || (cursor.offset == 0 && (cursor.section.is_none() || cursor.uri.is_some()))
    {
        return Err(Error::Invalid("page offset"));
    }
    Ok(cursor)
}

enum TrackPageKind {
    Playlist { can_edit_items: bool },
    SavedSongs,
}

fn valid_playlist_item_uid(uid: &str) -> bool {
    !uid.is_empty() && uid.len() <= 256 && uid.bytes().all(|byte| byte.is_ascii_graphic())
}

fn parse_playlist_tracks(data: &Value, cursor: Cursor) -> Result<Page<CatalogTrack>> {
    let can_edit_items = data["currentUserCapabilities"]["canEditItems"].as_bool() == Some(true);
    parse_track_page(
        &data["content"],
        cursor,
        TrackPageKind::Playlist { can_edit_items },
    )
}

fn parse_track_page(
    data: &Value,
    cursor: Cursor,
    kind: TrackPageKind,
) -> Result<Page<CatalogTrack>> {
    let next = next_cursor(data, cursor)?;
    let force_saved = matches!(kind, TrackPageKind::SavedSongs);
    let items = array(&data["items"])
        .iter()
        .filter_map(|item| {
            let (data, uri) = unwrap_item(item);
            let mut track = parse_track(data, uri, force_saved)?;
            if matches!(kind, TrackPageKind::Playlist { .. }) {
                track.added_at =
                    text(item, "/addedAt/isoString").or_else(|| text(item, "/addedAt"));
            }
            if matches!(
                kind,
                TrackPageKind::Playlist {
                    can_edit_items: true
                }
            ) {
                track.playlist_item_uid = item["uid"]
                    .as_str()
                    .filter(|uid| valid_playlist_item_uid(uid))
                    .map(str::to_owned);
            }
            Some(track)
        })
        .collect();
    Ok(Page { items, next })
}

fn parse_library_page(data: &Value, cursor: Cursor) -> Result<Page<CatalogItem>> {
    let next = next_cursor(data, cursor)?;
    Ok(Page {
        items: array(&data["items"])
            .iter()
            .filter_map(catalog_item)
            .collect(),
        next,
    })
}

fn parse_album_tracks(data: &Value, album: &AlbumSummary) -> Result<Vec<CatalogTrack>> {
    let items = data["tracksV2"]["items"]
        .as_array()
        .ok_or(Error::Invalid("album tracks"))?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let (data, uri) = unwrap_item(item);
            let mut track = parse_track(data, uri, false)?;
            track.metadata.album = Some(album.title.clone());
            track.metadata.cover_url = album.image.clone();
            track.album_uri = Some(album.uri.clone());
            if track.artists.is_empty() {
                track.artists = album.artists.clone();
                track.metadata.artist_names = track
                    .artists
                    .iter()
                    .map(|artist| artist.name.clone())
                    .collect();
            }
            Some(track)
        })
        .collect())
}

fn parse_artist(data: &Value, uri: &str, verified: bool) -> Result<Artist> {
    let summary = artist_summary(data, Some(uri)).ok_or(Error::Invalid("artist"))?;
    let top_tracks = array(&data["discography"]["topTracks"]["items"])
        .iter()
        .filter_map(|item| {
            let (data, uri) = unwrap_item(item);
            parse_track(data, uri, false)
        })
        .collect();
    let mut sections = Vec::new();
    for (key, title) in [
        ("albums", "Albums"),
        ("singles", "Singles & EPs"),
        ("compilations", "Compilations"),
        ("popularReleases", "Popular releases"),
    ] {
        let mut items = Vec::new();
        for item in array(&data["discography"][key]["items"]) {
            let release = array(&item["releases"]["items"]).first().unwrap_or(item);
            if let Some(mut album) = album_summary(release, None) {
                if album.artists.is_empty() {
                    album.artists.push(ArtistCredit {
                        name: summary.name.clone(),
                        uri: Some(summary.uri.clone()),
                    });
                }
                items.push(CatalogItem::Album(album));
            }
        }
        if !items.is_empty() {
            sections.push(Section {
                title: title.to_owned(),
                items,
            });
        }
    }
    for (key, title) in [
        ("appearsOn", "Appears on"),
        ("relatedArtists", "Fans also like"),
        ("discoveredOn", "Discovered on"),
    ] {
        let items: Vec<_> = array(&data["relatedContent"][key]["items"])
            .iter()
            .filter_map(|item| {
                let release = array(&item["releases"]["items"]).first().unwrap_or(item);
                catalog_item(release)
            })
            .collect();
        if !items.is_empty() {
            sections.push(Section {
                title: title.to_owned(),
                items,
            });
        }
    }
    Ok(Artist {
        summary,
        verified,
        description: text(data, "/profile/biography/text"),
        monthly_listeners: data["stats"]["monthlyListeners"].as_u64(),
        hero: image(&data["visuals"]["headerImage"]["sources"]),
        top_tracks,
        sections,
    })
}

fn parse_home(response: &Value) -> Result<Vec<Section>> {
    let sections = response
        .pointer("/data/home/sectionContainer/sections/items")
        .and_then(Value::as_array)
        .ok_or(Error::Invalid("home sections"))?;
    Ok(sections
        .iter()
        .filter_map(|section| {
            let title = ["transformedLabel", "translatedBaseText", "text"]
                .iter()
                .find_map(|key| text(&section["data"]["title"], &format!("/{key}")))?;
            let items: Vec<_> = array(&section["sectionItems"]["items"])
                .iter()
                .filter_map(catalog_item)
                .collect();
            (!items.is_empty()).then_some(Section { title, items })
        })
        .collect())
}

fn parse_search(response: &Value, keys: &[&str]) -> Result<Vec<CatalogItem>> {
    let search = response
        .pointer("/data/searchV2")
        .ok_or(Error::Invalid("search results"))?;
    let mut items = Vec::new();
    for key in keys {
        for value in array(&search[key]["itemsV2"])
            .iter()
            .chain(array(&search[key]["items"]).iter())
        {
            if let Some(item) = catalog_item(value)
                && !items
                    .iter()
                    .any(|existing| item_uri(existing) == item_uri(&item))
            {
                items.push(item);
            }
        }
    }
    Ok(items)
}

fn possible_recording_alias(left: &CatalogItem, right: &CatalogItem) -> bool {
    let (CatalogItem::Track(left), CatalogItem::Track(right)) = (left, right) else {
        return false;
    };
    left.metadata
        .title
        .eq_ignore_ascii_case(&right.metadata.title)
        && left.metadata.explicit == right.metadata.explicit
        && !left.artists.is_empty()
        && left.artists == right.artists
        && matches!(
            (left.metadata.duration_ms, right.metadata.duration_ms),
            (Some(left), Some(right)) if left > 0 && right > 0 && left.abs_diff(right) <= 3_000
        )
}

async fn collapse_search_recordings<F, Fut>(items: Vec<CatalogItem>, lookup: F) -> Vec<CatalogItem>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    let candidates = items
        .iter()
        .enumerate()
        .filter(|(index, item)| {
            items.iter().enumerate().any(|(other, candidate)| {
                *index != other && possible_recording_alias(item, candidate)
            })
        })
        .map(|(index, item)| (index, item_uri(item).to_owned()))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return items;
    }
    let mut lookups = stream::iter(candidates)
        .map(|(index, uri)| {
            let find = lookup(uri);
            async move { (index, find.await) }
        })
        .buffer_unordered(6);
    let mut recordings = vec![None; items.len()];
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some((index, canonical)) = lookups.next().await {
            recordings[index] = canonical;
        }
    })
    .await;
    let mut unique: Vec<(CatalogItem, Option<String>)> = Vec::with_capacity(items.len());
    for (item, canonical) in items.into_iter().zip(recordings) {
        if canonical.is_some()
            && unique.iter().any(|(existing, existing_canonical)| {
                canonical == *existing_canonical && possible_recording_alias(&item, existing)
            })
        {
            continue;
        }
        unique.push((item, canonical));
    }
    unique.into_iter().map(|(item, _)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotify_playlist_description_keeps_labels_and_decodes_entities() {
        let description = r#"<a href=spotify:playlist:twxn>TWXN</a>, <a href="spotify:playlist:swae">Swae Lee</a> &amp; friends<br>Made for you"#;
        assert_eq!(
            playlist_description(description.to_owned()).as_deref(),
            Some("TWXN, Swae Lee & friends\nMade for you")
        );
        assert_eq!(playlist_description("<br>".to_owned()), None);
        assert_eq!(
            playlist_description("A mix for me <3".to_owned()).as_deref(),
            Some("A mix for me <3")
        );
    }

    const ID: &str = "3TO7bbrUKrOSPGRTB5MeCz";
    const ARTIST: &str = "spotify:artist:74ASZWbe4lXaubB36ztrGX";
    const ALBUM: &str = "spotify:album:4aawyAB9vmqN3uQ7FjRGTy";

    fn track() -> Value {
        json!({
            "__typename": "Track", "name": "A song", "duration": {"totalMilliseconds": 192345},
            "artists": {"items": [{"uri": ARTIST, "profile": {"name": "A musician"}}]},
            "albumOfTrack": {"uri": ALBUM, "name": "A record", "coverArt": {"sources": [
                {"url": "https://i.scdn.co/image/small", "width": 64},
                {"url": "https://i.scdn.co/image/large", "width": 640},
                {"url": "http://example.test/insecure", "width": 2000}
            ]}},
            "contentRating": {"label": "EXPLICIT"}, "playability": {"playable": true}
        })
    }

    fn wrapped_track() -> Value {
        json!({"itemV2": {"__typename": "TrackResponseWrapper", "_uri": format!("spotify:track:{ID}"), "data": track()}})
    }

    #[test]
    fn links_keep_all_supported_resource_kinds() {
        for kind in ["track", "playlist", "album", "artist"] {
            let uri = format!("spotify:{kind}:{ID}");
            assert_eq!(resource_uri(&uri), Some(uri.clone()));
            assert_eq!(
                resource_uri(&format!(
                    "https://open.spotify.com/intl-ro/{kind}/{ID}?si=x"
                )),
                Some(uri)
            );
        }
        for input in [
            "https://open.spotify.com.evil.test/track/3TO7bbrUKrOSPGRTB5MeCz",
            "http://open.spotify.com/track/3TO7bbrUKrOSPGRTB5MeCz",
            "https://user@open.spotify.com/track/3TO7bbrUKrOSPGRTB5MeCz",
            "https://open.spotify.com/track/3TO7bbrUKrOSPGRTB5MeCz/extra",
            "spotify:episode:3TO7bbrUKrOSPGRTB5MeCz",
            "spotify:track:short",
        ] {
            assert!(resource_uri(input).is_none(), "{input}");
        }
    }

    #[test]
    fn playlist_paging_counts_unplayable_rows_and_keeps_links() {
        let mut playable = wrapped_track();
        playable["uid"] = json!("1234567890abcdef");
        playable["addedAt"] = json!({"isoString":"2026-10-27T12:34:56Z"});
        let mut unavailable = wrapped_track();
        unavailable["uid"] = json!("1234567890abcdee");
        unavailable["itemV2"]["data"]["playability"]["playable"] = json!(false);
        let page = parse_playlist_tracks(
            &json!({
                "currentUserCapabilities": {"canEditItems": true},
                "content": {"totalCount": 3, "items": [playable, unavailable]}
            }),
            Cursor {
                section: None,
                uri: Some(format!("spotify:playlist:{ID}")),
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(page.items.len(), 1);
        let track = &page.items[0];
        assert_eq!(track.metadata.uri, format!("spotify:track:{ID}"));
        assert_eq!(track.artists[0].uri.as_deref(), Some(ARTIST));
        assert_eq!(track.album_uri.as_deref(), Some(ALBUM));
        assert_eq!(
            track.metadata.cover_url.as_deref(),
            Some("https://i.scdn.co/image/large")
        );
        assert!(track.metadata.explicit);
        assert_eq!(track.playlist_item_uid.as_deref(), Some("1234567890abcdef"));
        assert_eq!(track.added_at.as_deref(), Some("2026-10-27T12:34:56Z"));
        assert_eq!(
            decode_cursor(page.next.as_deref().unwrap()).unwrap().offset,
            2
        );
    }

    #[test]
    fn only_editable_playlist_rows_keep_distinct_item_ids() {
        let mut first = wrapped_track();
        first["uid"] = json!("1234567890abcdef");
        let mut second = wrapped_track();
        second["uid"] = json!("1234567890abcdee");
        let mut invalid = wrapped_track();
        invalid["uid"] = json!(" ");
        let mut data = json!({
            "currentUserCapabilities": {"canEditItems": true},
            "content": {"totalCount": 53, "items": [first, second, invalid]}
        });
        let cursor = || Cursor {
            section: None,
            uri: Some(format!("spotify:playlist:{ID}")),
            offset: 50,
        };
        let editable = parse_playlist_tracks(&data, cursor()).unwrap();
        assert_eq!(
            editable.items[0].metadata.uri,
            editable.items[1].metadata.uri
        );
        assert_eq!(
            editable.items[0].playlist_item_uid.as_deref(),
            Some("1234567890abcdef")
        );
        assert_eq!(
            editable.items[1].playlist_item_uid.as_deref(),
            Some("1234567890abcdee")
        );
        assert_eq!(editable.items[2].playlist_item_uid, None);

        data["currentUserCapabilities"]["canEditItems"] = json!(false);
        let read_only = parse_playlist_tracks(&data, cursor()).unwrap();
        assert!(
            read_only
                .items
                .iter()
                .all(|track| track.playlist_item_uid.is_none())
        );
    }

    #[test]
    fn playlist_picker_keeps_membership_and_folders() {
        let playlist_uri = format!("spotify:playlist:{ID}");
        let data = json!({
            "totalCount": 7,
            "items": [
                {"item": {"data": {"__typename": "Playlist", "uri": playlist_uri, "name": "My list"}}, "curates": true},
                {"item": {"data": {"__typename": "PseudoPlaylist", "uri": "spotify:collection:tracks"}}, "curates": false},
                {"item": {"data": {"__typename": "PseudoPlaylist", "uri": "spotify:collection:your-episodes"}}, "curates": true},
                {"item": {"data": {"__typename": "Folder", "uri": "spotify:user:example:folder:123"}}},
                {"item": {"data": {"__typename": "Playlist", "uri": playlist_uri, "name": "Unknown membership"}}}
            ]
        });
        let page = parse_playlist_choices(&data, 1).unwrap();
        assert_eq!(page.liked, Some(false));
        assert_eq!(page.playlists.len(), 1);
        assert_eq!(page.playlists[0].0.uri, playlist_uri);
        assert!(page.playlists[0].1);
        assert_eq!(page.folders, ["spotify:user:example:folder:123"]);
        assert_eq!(page.next, Some(6));
        assert!(parse_playlist_choices(&json!({"totalCount": 2, "items": []}), 1).is_err());
    }

    #[test]
    fn saved_track_wrappers_mark_the_song_saved() {
        let page = parse_track_page(
            &json!({"totalCount": 1, "items": [{"track": {
                "_uri": format!("spotify:track:{ID}"), "data": track()
            }}]}),
            Cursor {
                section: Some(LibrarySection::Songs),
                uri: None,
                offset: 0,
            },
            TrackPageKind::SavedSongs,
        )
        .unwrap();
        assert!(page.items[0].saved);
        assert!(page.items[0].playlist_item_uid.is_none());
        assert!(page.next.is_none());
    }

    #[test]
    fn album_rows_receive_their_album_and_missing_credits() {
        let album = AlbumSummary {
            uri: ALBUM.to_owned(),
            title: "The album".to_owned(),
            artists: vec![ArtistCredit {
                name: "The artist".to_owned(),
                uri: Some(ARTIST.to_owned()),
            }],
            image: Some("https://i.scdn.co/image/cover".to_owned()),
            year: None,
            kind: None,
        };
        let tracks = parse_album_tracks(&json!({"tracksV2": {"items": [{"track": {
            "uri": format!("spotify:track:{ID}"), "name": "Opening", "duration": {"totalMilliseconds": 1000}
        }}]}}), &album).unwrap();
        assert_eq!(tracks[0].album_uri.as_deref(), Some(ALBUM));
        assert_eq!(tracks[0].metadata.album.as_deref(), Some("The album"));
        assert_eq!(tracks[0].artists, album.artists);
    }

    #[test]
    fn home_keeps_playlist_content_and_skips_unknown_cards() {
        let sections = parse_home(&json!({"data": {"home": {"sectionContainer": {"sections": {"items": [{
            "data": {"title": {"transformedLabel": "Made for you"}},
            "sectionItems": {"items": [
                {"content": {"__typename": "PlaylistResponseWrapper", "data": {
                    "__typename": "Playlist", "uri": format!("spotify:playlist:{ID}"), "name": "Daily mix", "content": {"totalCount": 30}
                }}},
                {"content": {"__typename": "PodcastResponseWrapper", "data": {"uri": "spotify:show:unsupported"}}}
            ]}
        }]}}}}})).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Made for you");
        assert_eq!(sections[0].items.len(), 1);
        assert!(
            matches!(&sections[0].items[0], CatalogItem::Playlist(playlist) if playlist.total == Some(30))
        );
    }

    #[test]
    fn search_keeps_ranked_results_once() {
        let wrapper = json!({"item": {"_uri": format!("spotify:track:{ID}"), "data": track()}});
        let results = parse_search(
            &json!({"data": {"searchV2": {
                "topResultsV2": {"itemsV2": [wrapper.clone()]}, "tracksV2": {"items": [wrapper]}
            }}}),
            &["topResultsV2", "tracksV2"],
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(item_uri(&results[0]), format!("spotify:track:{ID}"));
        assert!(parse_search(&json!({"data": {}}), &["tracksV2"]).is_err());
    }

    #[tokio::test]
    async fn search_collapses_only_confirmed_recording_aliases() {
        let uris = [1, 2, 3, 4, 5, 6].map(|id| format!("spotify:track:{id:022}"));
        let tracks = uris
            .iter()
            .enumerate()
            .map(|(index, uri)| {
                let mut data = track();
                data["duration"]["totalMilliseconds"] = json!(192_345 - index * 13);
                if index == 5 {
                    data["name"] = json!("A song - Live");
                }
                CatalogItem::Track(parse_track(&data, Some(uri), false).unwrap())
            })
            .collect();
        let results = collapse_search_recordings(tracks, |uri| {
            let index = uris.iter().position(|candidate| *candidate == uri).unwrap();
            let canonical = match index {
                0..=2 => Some(uris[1].clone()),
                3 => Some(uris[3].clone()),
                4 => None,
                _ => panic!("unrelated song should not need a lookup"),
            };
            std::future::ready(canonical)
        })
        .await;
        assert_eq!(
            results.iter().map(item_uri).collect::<Vec<_>>(),
            [&uris[0], &uris[3], &uris[4], &uris[5]]
        );
    }

    #[test]
    fn pages_reject_empty_partial_responses_and_foreign_cursors() {
        assert!(
            parse_track_page(
                &json!({"items": [], "totalCount": 10}),
                Cursor {
                    section: Some(LibrarySection::Songs),
                    uri: None,
                    offset: 0,
                },
                TrackPageKind::SavedSongs
            )
            .is_err()
        );
        assert!(decode_cursor("https://evil.test/?token=secret").is_err());
        let start = decode_cursor(&library_start_cursor(LibrarySection::Songs)).unwrap();
        assert_eq!(start.section, Some(LibrarySection::Songs));
        assert_eq!(start.offset, 0);
        assert!(decode_cursor("spotify-page:{\"section\":null,\"uri\":\"spotify:playlist:3TO7bbrUKrOSPGRTB5MeCz\",\"offset\":0}").is_err());
        assert!(decode_cursor("spotify-page:{\"section\":\"Songs\",\"uri\":null,\"offset\":1,\"url\":\"https://evil.test\"}").is_err());
    }
}
