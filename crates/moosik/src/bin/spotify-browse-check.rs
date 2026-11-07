use std::time::Duration;

use moosik::{
    api::LibrarySection,
    model::{Entry, SearchScope},
    spotify,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if !spotify::is_signed_in() {
        anyhow::bail!("Sign in to Spotify in Settings first");
    }
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Daft Punk".to_owned());
    let result = tokio::time::timeout(Duration::from_secs(180), check(&query)).await;
    match result {
        Ok(true) => Ok(()),
        Ok(false) => anyhow::bail!("Spotify browsing checks failed"),
        Err(_) => anyhow::bail!("Spotify browsing checks timed out"),
    }
}

async fn check(query: &str) -> bool {
    let mut passed = true;
    let mut artist = None;
    let mut album = None;
    let mut playlist = None;
    for scope in [
        SearchScope::All,
        SearchScope::Songs,
        SearchScope::Artists,
        SearchScope::Albums,
        SearchScope::Playlists,
    ] {
        match spotify::search_scope(query, scope).await {
            Ok(items) => {
                let tracks = items
                    .iter()
                    .filter(|item| matches!(item, Entry::Song(_)))
                    .count();
                let artists = items
                    .iter()
                    .filter(|item| matches!(item, Entry::Artist(_)))
                    .count();
                let albums = items
                    .iter()
                    .filter(|item| matches!(item, Entry::Album(_)))
                    .count();
                let playlists = items
                    .iter()
                    .filter(|item| matches!(item, Entry::Playlist(_)))
                    .count();
                println!(
                    "search_{}: OK songs={tracks} artists={artists} albums={albums} playlists={playlists}",
                    scope.label().to_lowercase()
                );
                if items.is_empty() {
                    passed = false;
                }
                for item in items {
                    match item {
                        Entry::Artist(item) if artist.is_none() => artist = Some(item.id),
                        Entry::Album(item) if album.is_none() => album = Some(item.id),
                        Entry::Playlist(item) if playlist.is_none() => playlist = Some(item.id),
                        _ => {}
                    }
                }
            }
            Err(error) => {
                println!("search_{}: FAIL {error}", scope.label().to_lowercase());
                passed = false;
            }
        }
    }
    if let Some(id) = artist {
        match spotify::artist(&id).await {
            Ok(artist) => println!(
                "artist: OK tracks={} shelves={}",
                artist.top_songs.len(),
                artist.shelves.len()
            ),
            Err(error) => {
                println!("artist: FAIL {error}");
                passed = false;
            }
        }
    } else {
        println!("artist: FAIL no search result");
        passed = false;
    }
    if let Some(id) = album {
        match spotify::album(&id).await {
            Ok(album) => println!("album: OK tracks={}", album.tracks.len()),
            Err(error) => {
                println!("album: FAIL {error}");
                passed = false;
            }
        }
    } else {
        println!("album: FAIL no search result");
        passed = false;
    }
    if let Some(id) = playlist {
        match spotify::playlist(&id).await {
            Ok(playlist) => {
                println!(
                    "playlist: OK tracks={} more={}",
                    playlist.tracks.items.len(),
                    playlist.tracks.next.is_some()
                );
                if let Some(next) = playlist.tracks.next {
                    match spotify::playlist_more(&next).await {
                        Ok(page) => println!(
                            "playlist_more: OK tracks={} more={}",
                            page.items.len(),
                            page.next.is_some()
                        ),
                        Err(error) => {
                            println!("playlist_more: FAIL {error}");
                            passed = false;
                        }
                    }
                }
            }
            Err(error) => {
                println!("playlist: FAIL {error}");
                passed = false;
            }
        }
    } else {
        println!("playlist: FAIL no search result");
        passed = false;
    }
    match spotify::home().await {
        Ok(home) => println!(
            "home: OK shelves={} items={}",
            home.shelves.len(),
            home.shelves
                .iter()
                .map(|shelf| shelf.items.len())
                .sum::<usize>()
        ),
        Err(error) => {
            println!("home: FAIL {error}");
            passed = false;
        }
    }
    match spotify::library().await {
        Ok(library) => {
            println!(
                "library: OK songs={} artists={} albums={} playlists={}",
                library.songs.items.len(),
                library.artists.items.len(),
                library.albums.items.len(),
                library.playlists.items.len()
            );
            for (name, section, next) in [
                ("songs", LibrarySection::Songs, library.songs.next),
                ("artists", LibrarySection::Artists, library.artists.next),
                ("albums", LibrarySection::Albums, library.albums.next),
                (
                    "playlists",
                    LibrarySection::Playlists,
                    library.playlists.next,
                ),
            ] {
                if let Some(next) = next {
                    match spotify::library_more(section, &next).await {
                        Ok(page) => println!(
                            "library_{name}_more: OK items={} more={}",
                            page.items.len(),
                            page.next.is_some()
                        ),
                        Err(error) => {
                            println!("library_{name}_more: FAIL {error}");
                            passed = false;
                        }
                    }
                }
            }
        }
        Err(error) => {
            println!("library: FAIL {error}");
            passed = false;
        }
    }
    passed
}
