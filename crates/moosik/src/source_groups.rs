use std::collections::{HashMap, HashSet};

use crate::model::{Entry, PlaylistSource, Provider};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchEvidence {
    Single,
    SongMetadata,
    AlbumMetadata,
    ArtistName,
    ArtistSongCredits,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub key: String,
    pub indices: Vec<usize>,
    pub variants: Vec<Entry>,
    pub evidence: MatchEvidence,
}

impl Group {
    pub fn preferred_index(
        &self,
        preferred: Provider,
        override_provider: Option<Provider>,
    ) -> Option<usize> {
        override_provider
            .and_then(|provider| {
                self.variants
                    .iter()
                    .position(|entry| entry_provider(entry) == provider)
            })
            .or_else(|| {
                self.variants
                    .iter()
                    .position(|entry| entry_provider(entry) == preferred)
            })
            .or_else(|| (!self.variants.is_empty()).then_some(0))
    }

    pub fn preferred_entry(
        &self,
        preferred: Provider,
        override_provider: Option<Provider>,
    ) -> Option<&Entry> {
        self.preferred_index(preferred, override_provider)
            .map(|index| &self.variants[index])
    }

    pub fn entry_for(&self, provider: Provider) -> Option<&Entry> {
        self.variants
            .iter()
            .find(|entry| entry_provider(entry) == provider)
    }

    pub fn providers(&self) -> Vec<Provider> {
        let mut providers = Vec::new();
        for entry in &self.variants {
            let provider = entry_provider(entry);
            if !providers.contains(&provider) {
                providers.push(provider);
            }
        }
        providers
    }

    pub fn is_combined(&self) -> bool {
        self.variants.len() > 1
    }

    pub fn artist_profiles(&self, id: &str) -> Option<(String, String)> {
        let youtube = unique_variant(self, Provider::YouTubeMusic)?;
        let spotify = unique_variant(self, Provider::Spotify)?;
        match (youtube, spotify) {
            (Entry::Artist(youtube), Entry::Artist(spotify)) => {
                let name = normalized(&youtube.name);
                if name.is_empty()
                    || name != normalized(&spotify.name)
                    || !remote_artist_id(Provider::YouTubeMusic, &youtube.id)
                    || !remote_artist_id(Provider::Spotify, &spotify.id)
                    || (id != youtube.id && id != spotify.id)
                {
                    return None;
                }
                Some((youtube.id.clone(), spotify.id.clone()))
            }
            (Entry::Song(youtube_track), Entry::Song(spotify_track)) => {
                let youtube_metadata = song_metadata(youtube)?;
                let spotify_metadata = song_metadata(spotify)?;
                let youtube_duration = youtube_metadata.duration?;
                let spotify_duration = spotify_metadata.duration?;
                if youtube_metadata.signature != spotify_metadata.signature
                    || youtube_duration.abs_diff(spotify_duration) > 3
                {
                    return None;
                }
                let youtube = unique_remote_credits(youtube_track)?;
                let spotify = unique_remote_credits(spotify_track)?;
                if youtube.len() != spotify.len()
                    || youtube.keys().any(|name| !spotify.contains_key(name))
                {
                    return None;
                }
                let mut pairs = youtube.iter().filter_map(|(name, youtube_id)| {
                    let spotify_id = spotify.get(name)?;
                    (id == *youtube_id || id == *spotify_id).then_some((*youtube_id, *spotify_id))
                });
                let (youtube, spotify) = pairs.next()?;
                if pairs.next().is_some() {
                    return None;
                }
                Some((youtube.to_owned(), spotify.to_owned()))
            }
            _ => None,
        }
    }
}

fn unique_variant(group: &Group, provider: Provider) -> Option<&Entry> {
    let mut variants = group
        .variants
        .iter()
        .filter(|entry| entry_provider(entry) == provider);
    let variant = variants.next()?;
    variants.next().is_none().then_some(variant)
}

fn remote_artist_id(provider: Provider, id: &str) -> bool {
    match provider {
        Provider::YouTubeMusic => {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }
        Provider::Spotify => id.strip_prefix("spotify:artist:").is_some_and(|id| {
            id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }),
        Provider::Local => false,
    }
}

fn unique_remote_credits(track: &crate::model::Track) -> Option<HashMap<String, &str>> {
    let provider = track.provider();
    let mut credits = HashMap::new();
    let mut ids = HashSet::new();
    for artist in &track.artists {
        let name = normalized(&artist.name);
        let id = artist.id.as_deref()?;
        if name.is_empty()
            || !remote_artist_id(provider, id)
            || !ids.insert(id)
            || credits.insert(name, id).is_some()
        {
            return None;
        }
    }
    (!credits.is_empty()).then_some(credits)
}

pub fn entry_provider(entry: &Entry) -> Provider {
    match entry {
        Entry::Song(track) | Entry::Video(track) => track.provider(),
        Entry::Artist(artist) if artist.id.starts_with("spotify:artist:") => Provider::Spotify,
        Entry::Album(album) if album.id.starts_with("spotify:album:") => Provider::Spotify,
        Entry::Playlist(playlist) => match playlist.source {
            PlaylistSource::YouTube => Provider::YouTubeMusic,
            PlaylistSource::Spotify => Provider::Spotify,
            PlaylistSource::Local => Provider::Local,
        },
        _ => Provider::YouTubeMusic,
    }
}

pub(crate) fn prioritize_entries(items: &mut Vec<Entry>, preferred: Option<Provider>) {
    let Some(first) = items.first().map(entry_provider) else {
        return;
    };
    let primary = preferred
        .filter(|provider| items.iter().any(|entry| entry_provider(entry) == *provider))
        .unwrap_or(first);
    if items.iter().all(|entry| entry_provider(entry) == primary) {
        return;
    }
    let weight = if preferred == Some(primary) { 2 } else { 1 };
    let (primary_entries, other_entries): (Vec<_>, Vec<_>) = std::mem::take(items)
        .into_iter()
        .partition(|entry| entry_provider(entry) == primary);
    let mut primary = primary_entries.into_iter();
    for other in other_entries {
        items.extend(primary.by_ref().take(weight));
        items.push(other);
    }
    items.extend(primary);
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SongSignature {
    title: String,
    artists: Vec<String>,
    explicit: bool,
}

struct SongMetadata {
    signature: SongSignature,
    duration: Option<u64>,
    provider: Provider,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct AlbumSignature {
    title: String,
    artists: Vec<String>,
    year: u16,
    kind: String,
}

struct AlbumMetadata {
    signature: AlbumSignature,
    explicit: Option<bool>,
    provider: Provider,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ArtistIdentity {
    provider: Provider,
    id: String,
}

type ArtistLinks = HashMap<ArtistIdentity, HashSet<ArtistIdentity>>;

struct Edge {
    left: usize,
    right: usize,
    priority: u64,
}

pub fn group_entries(items: &[Entry], enabled: bool) -> Vec<Group> {
    let identities: Vec<_> = items.iter().map(entry_identity).collect();
    let mut members: Vec<_> = (0..items.len()).map(|index| vec![index]).collect();
    let mut owners: Vec<_> = (0..items.len()).collect();
    let mut artist_links = ArtistLinks::new();
    if enabled {
        let metadata: Vec<_> = items.iter().map(song_metadata).collect();
        let edges = song_edges(&metadata);
        merge_edges(edges, &identities, &mut members, &mut owners);
        artist_links = matched_artist_links(items, &members);
        let edges = artist_edges(items, &artist_links);
        merge_edges(edges, &identities, &mut members, &mut owners);
        let metadata: Vec<_> = items.iter().map(album_metadata).collect();
        let edges = album_edges(&metadata);
        merge_edges(edges, &identities, &mut members, &mut owners);
    }
    members.into_iter().filter(|members| !members.is_empty()).map(|mut members| {
        members.sort_unstable();
        let variants: Vec<_> = members.iter().map(|index| items[*index].clone()).collect();
        let mut keys: Vec<_> = members.iter().map(|index| identities[*index].as_str()).collect();
        keys.sort_unstable();
        let key = encoded_parts(keys);
        let evidence = if variants.len() == 1 {
            MatchEvidence::Single
        } else if matches!(variants[0], Entry::Song(_)) {
            MatchEvidence::SongMetadata
        } else if matches!(variants[0], Entry::Album(_)) {
            MatchEvidence::AlbumMetadata
        } else if variants.iter().enumerate().all(|(index, entry)| variants[index + 1..].iter().all(|other| {
            let left = artist_identity(entry);
            let right = artist_identity(other);
            matches!((left, right), (Some(left), Some(right)) if artist_links.get(&left).is_some_and(|links| links.contains(&right)))
        })) {
            MatchEvidence::ArtistSongCredits
        } else {
            MatchEvidence::ArtistName
        };
        Group { key, indices: members, variants, evidence }
    }).collect()
}

fn normalized(value: &str) -> String {
    let normalizer = icu_normalizer::ComposingNormalizerBorrowed::new_nfc();
    normalizer
        .normalize(value)
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalized_song_title(value: &str) -> String {
    let title = normalized(value);
    if let Some((base, version)) = title
        .strip_suffix(')')
        .and_then(|title| title.rsplit_once(" ("))
        && !version.trim().is_empty()
        && !version.contains(['(', ')'])
    {
        return format!("{base} - {}", version.trim());
    }
    title
}

fn song_metadata(entry: &Entry) -> Option<SongMetadata> {
    let Entry::Song(track) = entry else {
        return None;
    };
    if track.wide || track.artists.is_empty() || track.video_id.is_empty() {
        return None;
    }
    let title = normalized_song_title(&track.title);
    let mut artists: Vec<_> = track
        .artists
        .iter()
        .map(|artist| normalized(&artist.name))
        .collect();
    if title.is_empty() || artists.iter().any(String::is_empty) {
        return None;
    }
    artists.sort_unstable();
    artists.dedup();
    Some(SongMetadata {
        signature: SongSignature {
            title,
            artists,
            explicit: track.explicit,
        },
        duration: track.duration.filter(|duration| *duration > 0),
        provider: track.provider(),
    })
}

fn song_edges(metadata: &[Option<SongMetadata>]) -> Vec<Edge> {
    let mut buckets: HashMap<&SongSignature, Vec<usize>> = HashMap::new();
    for (index, song) in metadata.iter().enumerate() {
        if let Some(song) = song {
            buckets.entry(&song.signature).or_default().push(index);
        }
    }
    let mut edges = Vec::new();
    for indices in buckets.values() {
        for (position, left) in indices.iter().enumerate() {
            let song = metadata[*left].as_ref().expect("song bucket");
            for right in &indices[position + 1..] {
                let other = metadata[*right].as_ref().expect("song bucket");
                if song.provider == other.provider {
                    continue;
                }
                let priority = match (song.duration, other.duration) {
                    (Some(duration), Some(other_duration)) => {
                        let distance = duration.abs_diff(other_duration);
                        let unique = |duration: u64, provider| {
                            indices
                                .iter()
                                .filter(|index| {
                                    let candidate =
                                        metadata[**index].as_ref().expect("song bucket");
                                    candidate.provider == provider
                                        && candidate
                                            .duration
                                            .is_some_and(|value| duration.abs_diff(value) <= 3)
                                })
                                .count()
                                == 1
                        };
                        if distance > 3
                            || !unique(duration, other.provider)
                            || !unique(other_duration, song.provider)
                        {
                            continue;
                        }
                        distance
                    }
                    _ => {
                        let fallback = matches!(
                            (song.provider, song.duration, other.provider, other.duration),
                            (Provider::YouTubeMusic, None, Provider::Spotify, Some(_))
                                | (Provider::Spotify, Some(_), Provider::YouTubeMusic, None)
                        );
                        let unique = |provider| {
                            indices
                                .iter()
                                .filter(|index| {
                                    metadata[**index].as_ref().expect("song bucket").provider
                                        == provider
                                })
                                .count()
                                == 1
                        };
                        if !fallback || !unique(song.provider) || !unique(other.provider) {
                            continue;
                        }
                        4
                    }
                };
                edges.push(Edge {
                    left: *left,
                    right: *right,
                    priority,
                });
            }
        }
    }
    edges
}

fn album_metadata(entry: &Entry) -> Option<AlbumMetadata> {
    let Entry::Album(album) = entry else {
        return None;
    };
    let provider = entry_provider(entry);
    let valid_id = match provider {
        Provider::YouTubeMusic => {
            !album.id.is_empty()
                && album
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }
        Provider::Spotify => album.id.strip_prefix("spotify:album:").is_some_and(|id| {
            id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }),
        Provider::Local => false,
    };
    if !valid_id {
        return None;
    }
    let title = normalized(&album.title);
    let mut artists = if album.artists.is_empty() {
        vec![normalized(&album.artist)]
    } else {
        album
            .artists
            .iter()
            .map(|artist| normalized(&artist.name))
            .collect()
    };
    if title.is_empty()
        || artists.iter().any(String::is_empty)
        || artists.iter().any(|artist| artist == "various artists")
    {
        return None;
    }
    artists.sort_unstable();
    artists.dedup();
    let year = album.year.as_deref()?.trim();
    if year.len() != 4 || !year.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year = year.parse::<u16>().ok().filter(|year| *year > 0)?;
    let kind = normalized(album.kind.as_deref()?);
    if !matches!(kind.as_str(), "album" | "single" | "ep" | "compilation") {
        return None;
    }
    Some(AlbumMetadata {
        signature: AlbumSignature {
            title,
            artists,
            year,
            kind,
        },
        explicit: album.explicit,
        provider,
    })
}

fn album_edges(metadata: &[Option<AlbumMetadata>]) -> Vec<Edge> {
    let mut buckets: HashMap<&AlbumSignature, Vec<usize>> = HashMap::new();
    for (index, album) in metadata.iter().enumerate() {
        if let Some(album) = album {
            buckets.entry(&album.signature).or_default().push(index);
        }
    }
    let mut edges = Vec::new();
    for indices in buckets.values() {
        for (position, left) in indices.iter().enumerate() {
            let album = metadata[*left].as_ref().expect("album bucket");
            for right in &indices[position + 1..] {
                let other = metadata[*right].as_ref().expect("album bucket");
                let compatible = |left: &AlbumMetadata, right: &AlbumMetadata| {
                    left.provider != right.provider
                        && !matches!((left.explicit, right.explicit), (Some(left), Some(right)) if left != right)
                };
                let unique = |album: &AlbumMetadata, provider| {
                    indices
                        .iter()
                        .filter(|index| {
                            let candidate = metadata[**index].as_ref().expect("album bucket");
                            candidate.provider == provider && compatible(album, candidate)
                        })
                        .count()
                        == 1
                };
                if !compatible(album, other)
                    || !unique(album, other.provider)
                    || !unique(other, album.provider)
                {
                    continue;
                }
                edges.push(Edge {
                    left: *left,
                    right: *right,
                    priority: u64::from(album.explicit.is_none() || other.explicit.is_none()),
                });
            }
        }
    }
    edges
}

fn artist_edges(items: &[Entry], links: &ArtistLinks) -> Vec<Edge> {
    let mut counts = HashMap::new();
    for entry in items {
        if let Entry::Artist(artist) = entry {
            let name = normalized(&artist.name);
            if !name.is_empty() && !artist.id.is_empty() {
                *counts
                    .entry((entry_provider(entry), name))
                    .or_insert(0usize) += 1;
            }
        }
    }
    let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, entry) in items.iter().enumerate() {
        if let Entry::Artist(artist) = entry {
            let name = normalized(&artist.name);
            if !artist.id.is_empty()
                && counts.get(&(entry_provider(entry), name.clone())) == Some(&1)
            {
                buckets.entry(name).or_default().push(index);
            }
        }
    }
    let mut edges = Vec::new();
    for indices in buckets.values() {
        for (position, left) in indices.iter().enumerate() {
            let left_id = artist_identity(&items[*left]).expect("artist bucket");
            for right in &indices[position + 1..] {
                let right_id = artist_identity(&items[*right]).expect("artist bucket");
                if left_id.provider == right_id.provider
                    || !artist_link_allows(links, &left_id, &right_id)
                    || !artist_link_allows(links, &right_id, &left_id)
                {
                    continue;
                }
                let linked = links
                    .get(&left_id)
                    .is_some_and(|targets| targets.contains(&right_id));
                edges.push(Edge {
                    left: *left,
                    right: *right,
                    priority: u64::from(!linked),
                });
            }
        }
    }
    edges
}

fn artist_link_allows(links: &ArtistLinks, from: &ArtistIdentity, to: &ArtistIdentity) -> bool {
    let Some(targets) = links.get(from) else {
        return true;
    };
    let mut same_provider = targets
        .iter()
        .filter(|target| target.provider == to.provider);
    match (same_provider.next(), same_provider.next()) {
        (None, _) => true,
        (Some(target), None) => target == to,
        _ => false,
    }
}

fn merge_edges(
    mut edges: Vec<Edge>,
    identities: &[String],
    members: &mut [Vec<usize>],
    owners: &mut [usize],
) {
    let pair_key = |edge: &Edge| {
        let left = identities[edge.left].as_str();
        let right = identities[edge.right].as_str();
        if left <= right {
            (left, right)
        } else {
            (right, left)
        }
    };
    edges.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| pair_key(left).cmp(&pair_key(right)))
    });
    let pairs: HashSet<_> = edges
        .iter()
        .map(|edge| ordered_pair(edge.left, edge.right))
        .collect();
    for edge in edges {
        let left = owners[edge.left];
        let right = owners[edge.right];
        if left == right
            || !members[left].iter().all(|left| {
                members[right]
                    .iter()
                    .all(|right| pairs.contains(&ordered_pair(*left, *right)))
            })
        {
            continue;
        }
        let keep = left.min(right);
        let remove = left.max(right);
        let moved = std::mem::take(&mut members[remove]);
        for index in &moved {
            owners[*index] = keep;
        }
        members[keep].extend(moved);
    }
}

fn ordered_pair(left: usize, right: usize) -> (usize, usize) {
    (left.min(right), left.max(right))
}

fn artist_identity(entry: &Entry) -> Option<ArtistIdentity> {
    let Entry::Artist(artist) = entry else {
        return None;
    };
    (!artist.id.is_empty()).then(|| ArtistIdentity {
        provider: entry_provider(entry),
        id: artist.id.clone(),
    })
}

fn matched_artist_links(items: &[Entry], members: &[Vec<usize>]) -> ArtistLinks {
    let mut links = ArtistLinks::new();
    for group in members.iter().filter(|group| group.len() > 1) {
        for (position, left) in group.iter().enumerate() {
            let Entry::Song(left) = &items[*left] else {
                continue;
            };
            let Some(left_duration) = left.duration.filter(|duration| *duration > 0) else {
                continue;
            };
            for right in &group[position + 1..] {
                let Entry::Song(right) = &items[*right] else {
                    continue;
                };
                let Some(right_duration) = right.duration.filter(|duration| *duration > 0) else {
                    continue;
                };
                if left_duration.abs_diff(right_duration) > 3 {
                    continue;
                }
                for left_artist in &left.artists {
                    let Some(left_id) = left_artist.id.as_ref().filter(|id| !id.is_empty()) else {
                        continue;
                    };
                    for right_artist in &right.artists {
                        let Some(right_id) = right_artist.id.as_ref().filter(|id| !id.is_empty())
                        else {
                            continue;
                        };
                        if normalized(&left_artist.name) == normalized(&right_artist.name) {
                            let left = ArtistIdentity {
                                provider: left.provider(),
                                id: left_id.clone(),
                            };
                            let right = ArtistIdentity {
                                provider: right.provider(),
                                id: right_id.clone(),
                            };
                            links.entry(left.clone()).or_default().insert(right.clone());
                            links.entry(right).or_default().insert(left);
                        }
                    }
                }
            }
        }
    }
    links
}

fn encoded_parts<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut key = String::new();
    for part in parts {
        key.push_str(&part.len().to_string());
        key.push(':');
        key.push_str(part);
    }
    key
}

fn entry_identity(entry: &Entry) -> String {
    let provider = match entry_provider(entry) {
        Provider::YouTubeMusic => "youtube",
        Provider::Spotify => "spotify",
        Provider::Local => "local",
    };
    let (kind, id) = match entry {
        Entry::Song(track) => ("song", track.video_id.as_str()),
        Entry::Video(track) => ("video", track.video_id.as_str()),
        Entry::Artist(artist) => ("artist", artist.id.as_str()),
        Entry::Album(album) => ("album", album.id.as_str()),
        Entry::Playlist(playlist) => ("playlist", playlist.id.as_str()),
        Entry::Mood(mood) => ("mood", mood.params.as_str()),
    };
    encoded_parts([provider, kind, id])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AlbumRef, ArtistActions, ArtistLink, ArtistRef};

    fn song(
        provider: Provider,
        id: &str,
        title: &str,
        artists: &[&str],
        duration: Option<u64>,
    ) -> Entry {
        let mut track = crate::listening::Metadata {
            id: id.to_owned(),
            title: title.to_owned(),
            ..Default::default()
        }
        .to_track();
        track.artists = artists
            .iter()
            .map(|name| ArtistLink::plain(*name))
            .collect();
        track.duration = duration;
        match provider {
            Provider::Spotify => {
                let id: String = id.chars().filter(char::is_ascii_alphanumeric).collect();
                track.video_id = format!("spotify:track:{id:0<22}");
            }
            Provider::Local => track.local_path = Some(std::path::PathBuf::from(id).into()),
            Provider::YouTubeMusic => {}
        }
        Entry::Song(track)
    }

    fn artist(provider: Provider, id: &str, name: &str) -> Entry {
        Entry::Artist(ArtistRef {
            id: if provider == Provider::Spotify {
                format!("spotify:artist:{id}")
            } else {
                id.to_owned()
            },
            name: name.to_owned(),
            subscribers: None,
            thumbnail: None,
            actions: ArtistActions::default(),
        })
    }

    fn album(
        provider: Provider,
        id: &str,
        title: &str,
        artists: &[&str],
        year: Option<&str>,
        kind: Option<&str>,
        explicit: Option<bool>,
    ) -> Entry {
        Entry::Album(AlbumRef {
            id: if provider == Provider::Spotify {
                format!("spotify:album:{id:0<22}")
            } else {
                format!("MPREb_{id}")
            },
            title: title.to_owned(),
            artist: artists.join(", "),
            artists: artists
                .iter()
                .map(|name| ArtistLink::plain(*name))
                .collect(),
            explicit,
            year: year.map(str::to_owned),
            kind: kind.map(str::to_owned),
            thumbnail: None,
        })
    }

    fn matching_album(provider: Provider, id: &str) -> Entry {
        album(
            provider,
            id,
            "A Record",
            &["Artist"],
            Some("2024"),
            Some("Album"),
            Some(false),
        )
    }

    #[test]
    fn matching_albums_keep_original_indices_variants_and_source_preference() {
        let items = vec![
            matching_album(Provider::YouTubeMusic, "yt"),
            song(
                Provider::YouTubeMusic,
                "song",
                "Song",
                &["Artist"],
                Some(180),
            ),
            matching_album(Provider::Spotify, "sp"),
        ];
        let original = items.clone();
        let groups = group_entries(&items, true);
        assert_eq!(items, original);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].indices, [0, 2]);
        assert_eq!(groups[0].variants, [items[0].clone(), items[2].clone()]);
        assert_eq!(groups[0].evidence, MatchEvidence::AlbumMetadata);
        assert_eq!(groups[1].indices, [1]);
        let preferred = groups[0].preferred_index(Provider::Spotify, None).unwrap();
        assert_eq!(groups[0].indices[preferred], 2);
        assert_eq!(
            groups[0].preferred_entry(Provider::Spotify, Some(Provider::YouTubeMusic)),
            Some(&items[0])
        );
        assert_eq!(group_entries(&items, false).len(), 3);
        let reversed = group_entries(&[items[2].clone(), items[0].clone()], true);
        assert_eq!(groups[0].key, reversed[0].key);
        assert_eq!(reversed[0].indices, [0, 1]);
    }

    #[test]
    fn album_editions_and_title_punctuation_stay_separate() {
        let youtube = matching_album(Provider::YouTubeMusic, "yt");
        for title in [
            "A Record (Deluxe)",
            "A Record (Remastered)",
            "A Record (Live)",
            "A Record (Clean)",
            "A Record (Anniversary Edition)",
            "A Record (Mono)",
            "A Record!",
        ] {
            let spotify = album(
                Provider::Spotify,
                "sp",
                title,
                &["Artist"],
                Some("2024"),
                Some("Album"),
                Some(false),
            );
            assert_eq!(group_entries(&[youtube.clone(), spotify], true).len(), 2);
        }
        let mut youtube = youtube;
        let Entry::Album(album) = &mut youtube else {
            unreachable!()
        };
        album.title = "A Record (Deluxe)".to_owned();
        let mut spotify = matching_album(Provider::Spotify, "sp");
        let Entry::Album(album) = &mut spotify else {
            unreachable!()
        };
        album.title = " a  record (deluxe) ".to_owned();
        assert_eq!(group_entries(&[youtube, spotify], true).len(), 1);
    }

    #[test]
    fn albums_require_known_matching_artist_year_kind_and_compatible_explicitness() {
        let youtube = matching_album(Provider::YouTubeMusic, "yt");
        let Entry::Album(spotify) = matching_album(Provider::Spotify, "sp") else {
            unreachable!()
        };
        let mut invalid = Vec::new();
        let mut changed = spotify.clone();
        changed.artists[0].name = "Other Artist".to_owned();
        invalid.push(changed);
        let mut changed = spotify.clone();
        changed.artists[0].name.clear();
        invalid.push(changed);
        let mut changed = spotify.clone();
        changed.artists.clear();
        changed.artist.clear();
        invalid.push(changed);
        let mut changed = spotify.clone();
        changed.explicit = Some(true);
        invalid.push(changed);
        for year in [
            None,
            Some("2023"),
            Some("24"),
            Some("2024-01-01"),
            Some("0000"),
        ] {
            let mut changed = spotify.clone();
            changed.year = year.map(str::to_owned);
            invalid.push(changed);
        }
        for kind in [None, Some("Single"), Some("EP"), Some("Other")] {
            let mut changed = spotify.clone();
            changed.kind = kind.map(str::to_owned);
            invalid.push(changed);
        }
        for changed in invalid {
            assert_eq!(
                group_entries(&[youtube.clone(), Entry::Album(changed)], true).len(),
                2
            );
        }
        let mut unknown = spotify;
        unknown.explicit = None;
        assert_eq!(
            group_entries(&[youtube, Entry::Album(unknown)], true).len(),
            1
        );
    }

    #[test]
    fn competing_album_editions_require_one_compatible_candidate_per_source() {
        let clean = matching_album(Provider::YouTubeMusic, "clean");
        let mut explicit = matching_album(Provider::YouTubeMusic, "explicit");
        let Entry::Album(album) = &mut explicit else {
            unreachable!()
        };
        album.explicit = Some(true);
        let mut spotify = matching_album(Provider::Spotify, "sp");
        let groups = group_entries(&[clean.clone(), explicit.clone(), spotify.clone()], true);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].indices, [0, 2]);
        assert_eq!(groups[1].indices, [1]);
        let Entry::Album(album) = &mut spotify else {
            unreachable!()
        };
        album.explicit = None;
        assert_eq!(
            group_entries(&[clean.clone(), explicit, spotify.clone()], true).len(),
            3
        );
        let other_clean = matching_album(Provider::YouTubeMusic, "other");
        assert_eq!(group_entries(&[clean, other_clean, spotify], true).len(), 3);
    }

    #[test]
    fn album_credit_boundaries_are_preserved_and_credit_order_can_differ() {
        let youtube = album(
            Provider::YouTubeMusic,
            "yt",
            "Record",
            &["Ann", "Bo"],
            Some("2024"),
            Some("Album"),
            None,
        );
        let spotify = album(
            Provider::Spotify,
            "sp",
            "record",
            &[" BO ", "ann"],
            Some("2024"),
            Some("ALBUM"),
            None,
        );
        assert_eq!(group_entries(&[youtube.clone(), spotify], true).len(), 1);
        let combined_name = album(
            Provider::Spotify,
            "sp",
            "Record",
            &["Ann, Bo"],
            Some("2024"),
            Some("Album"),
            None,
        );
        assert_eq!(group_entries(&[youtube, combined_name], true).len(), 2);
        let mut fallback = matching_album(Provider::YouTubeMusic, "yt");
        let Entry::Album(album) = &mut fallback else {
            unreachable!()
        };
        album.artists.clear();
        assert_eq!(
            group_entries(&[fallback, matching_album(Provider::Spotify, "sp")], true).len(),
            1
        );
    }

    #[test]
    fn album_grouping_requires_distinct_sources_and_valid_native_ids() {
        let youtube = matching_album(Provider::YouTubeMusic, "yt");
        let same_source = matching_album(Provider::YouTubeMusic, "other");
        assert_eq!(
            group_entries(&[youtube.clone(), same_source], true).len(),
            2
        );
        for id in [
            "",
            "bad/id",
            "spotify:album:short",
            "spotify:artist:1234567890123456789012",
            "spotify:album:123456789012345678901!",
        ] {
            let mut invalid = matching_album(Provider::Spotify, "sp");
            let Entry::Album(album) = &mut invalid else {
                unreachable!()
            };
            album.id = id.to_owned();
            assert_eq!(group_entries(&[youtube.clone(), invalid], true).len(), 2);
        }
    }

    #[test]
    fn various_artist_compilations_do_not_merge_by_generic_credits() {
        let youtube = album(
            Provider::YouTubeMusic,
            "yt",
            "Greatest Hits",
            &["Various Artists"],
            Some("2024"),
            Some("Compilation"),
            None,
        );
        let spotify = album(
            Provider::Spotify,
            "sp",
            "Greatest Hits",
            &["Various Artists"],
            Some("2024"),
            Some("Compilation"),
            None,
        );
        assert_eq!(group_entries(&[youtube, spotify], true).len(), 2);
    }

    #[test]
    fn matching_songs_keep_rank_and_native_variants() {
        let items = vec![
            song(
                Provider::YouTubeMusic,
                "yt",
                "  One More Time ",
                &["Daft Punk", "Guest"],
                Some(320),
            ),
            song(
                Provider::Spotify,
                "sp",
                "one more time",
                &["GUEST", "Daft  Punk"],
                Some(323),
            ),
            song(
                Provider::Local,
                "local.flac",
                "One More Time",
                &["Daft Punk", "Guest"],
                Some(321),
            ),
        ];
        let groups = group_entries(&items, true);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].variants, items);
        assert_eq!(groups[0].evidence, MatchEvidence::SongMetadata);
        assert_eq!(groups[0].preferred_index(Provider::Spotify, None), Some(1));
        assert_eq!(
            groups[0].preferred_index(Provider::Spotify, Some(Provider::Local)),
            Some(2)
        );
        assert!(
            groups[0]
                .entry_for(Provider::Local)
                .unwrap()
                .track()
                .unwrap()
                .local_path
                .is_some()
        );
        assert_eq!(group_entries(&items, false).len(), 3);
    }

    #[test]
    fn youtube_artist_songs_without_durations_keep_rank_and_native_variants() {
        let items = vec![
            song(
                Provider::YouTubeMusic,
                "yt-woman",
                "  Woman ",
                &["Doja Cat"],
                None,
            ),
            song(
                Provider::Spotify,
                "sp-woman",
                "woman",
                &["Doja  Cat"],
                Some(173),
            ),
            song(
                Provider::YouTubeMusic,
                "yt-streets",
                "Streets",
                &["Doja Cat"],
                Some(0),
            ),
            song(
                Provider::Spotify,
                "sp-streets",
                "Streets",
                &["DOJA CAT"],
                Some(226),
            ),
        ];
        let groups = group_entries(&items, true);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].variants, items[..2]);
        assert_eq!(groups[1].variants, items[2..]);
        assert_eq!(groups[0].evidence, MatchEvidence::SongMetadata);
        assert_eq!(groups[0].preferred_index(Provider::Spotify, None), Some(1));
        let reversed = group_entries(&[items[1].clone(), items[0].clone()], true);
        assert_eq!(groups[0].key, reversed[0].key);
        assert_eq!(
            groups[0].preferred_entry(Provider::Spotify, None),
            reversed[0].preferred_entry(Provider::Spotify, None)
        );
        assert_eq!(group_entries(&items, false).len(), 4);
    }

    #[test]
    fn duration_fallback_requires_youtube_missing_and_spotify_known() {
        for (left_provider, left_duration, right_provider, right_duration) in [
            (Provider::YouTubeMusic, None, Provider::Spotify, None),
            (Provider::YouTubeMusic, None, Provider::Spotify, Some(0)),
            (Provider::YouTubeMusic, Some(200), Provider::Spotify, None),
            (Provider::Local, None, Provider::Spotify, Some(200)),
            (Provider::YouTubeMusic, None, Provider::Local, Some(200)),
            (Provider::YouTubeMusic, Some(200), Provider::Local, None),
        ] {
            let items = [
                song(left_provider, "left", "Song", &["Artist"], left_duration),
                song(right_provider, "right", "Song", &["Artist"], right_duration),
            ];
            assert_eq!(group_entries(&items, true).len(), 2);
        }
    }

    #[test]
    fn duration_fallback_requires_unique_candidates_from_both_providers() {
        let youtube = song(Provider::YouTubeMusic, "yt", "Song", &["Artist"], None);
        let spotify = song(Provider::Spotify, "sp", "Song", &["Artist"], Some(200));
        for (provider, duration) in [
            (Provider::YouTubeMusic, None),
            (Provider::YouTubeMusic, Some(0)),
            (Provider::YouTubeMusic, Some(500)),
            (Provider::Spotify, None),
            (Provider::Spotify, Some(0)),
            (Provider::Spotify, Some(500)),
        ] {
            let alternative = song(provider, "alternative", "Song", &["Artist"], duration);
            let items = [youtube.clone(), spotify.clone(), alternative];
            assert_eq!(group_entries(&items, true).len(), 3);
        }
        let known = song(
            Provider::YouTubeMusic,
            "known-yt",
            "Song",
            &["Artist"],
            Some(200),
        );
        let groups = group_entries(&[youtube.clone(), spotify.clone(), known.clone()], true);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].variants, [youtube]);
        assert_eq!(groups[1].variants, [spotify, known]);
    }

    #[test]
    fn duration_fallback_keeps_versions_credits_and_explicitness_distinct() {
        let youtube = song(Provider::YouTubeMusic, "yt", "Song", &["Artist"], None);
        for title in [
            "Song (Live)",
            "Song (Remix)",
            "Song - 2020 Remaster",
            "Song (Clean)",
        ] {
            let spotify = song(Provider::Spotify, "sp", title, &["Artist"], Some(200));
            assert_eq!(group_entries(&[youtube.clone(), spotify], true).len(), 2);
        }
        for artists in [vec![], vec![""], vec!["Artist", "Guest"]] {
            let spotify = song(Provider::Spotify, "sp", "Song", &artists, Some(200));
            assert_eq!(group_entries(&[youtube.clone(), spotify], true).len(), 2);
        }
        let mut spotify = song(Provider::Spotify, "sp", "Song", &["Artist"], Some(200));
        spotify.track_mut().unwrap().explicit = true;
        assert_eq!(group_entries(&[youtube.clone(), spotify], true).len(), 2);
        let mut video = youtube;
        video.track_mut().unwrap().wide = true;
        let spotify = song(Provider::Spotify, "sp", "Song", &["Artist"], Some(200));
        assert_eq!(group_entries(&[video, spotify], true).len(), 2);
    }

    #[test]
    fn song_versions_match_across_suffix_punctuation() {
        let credits = ["Lele", "Petre Stefan", "Manele VTM"];
        let youtube = song(
            Provider::YouTubeMusic,
            "yt",
            "Peter Parker (Balkanic Version)",
            &credits,
            Some(199),
        );
        let spotify = song(
            Provider::Spotify,
            "sp",
            "Peter Parker - Balkanic Version",
            &credits,
            Some(198),
        );
        let remix = song(
            Provider::Spotify,
            "remix",
            "Peter Parker - Live",
            &credits,
            Some(198),
        );
        let groups = group_entries(&[youtube.clone(), spotify.clone(), remix.clone()], true);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].variants, [youtube, spotify]);
        assert_eq!(groups[1].variants, [remix]);
    }

    #[test]
    fn missing_credits_versions_explicitness_and_duration_stay_separate() {
        let base = song(Provider::YouTubeMusic, "yt", "Song", &["Artist"], Some(200));
        for candidate in [
            song(
                Provider::Spotify,
                "sp",
                "Song (Live)",
                &["Artist"],
                Some(200),
            ),
            song(
                Provider::Spotify,
                "sp",
                "Song - 2020 Remaster",
                &["Artist"],
                Some(200),
            ),
            song(
                Provider::Spotify,
                "sp",
                "Song (Remix)",
                &["Artist"],
                Some(200),
            ),
            song(
                Provider::Spotify,
                "sp",
                "Song",
                &["Artist", "Guest"],
                Some(200),
            ),
            song(Provider::Spotify, "sp", "Song", &["Artist"], None),
            song(Provider::Spotify, "sp", "Song", &[], Some(200)),
            song(Provider::Spotify, "sp", "Song", &[""], Some(200)),
            song(Provider::Spotify, "sp", "Song", &["Artist"], Some(204)),
        ] {
            assert_eq!(group_entries(&[base.clone(), candidate], true).len(), 2);
        }
        let mut explicit = song(Provider::Spotify, "sp", "Song", &["Artist"], Some(200));
        explicit.track_mut().unwrap().explicit = true;
        assert_eq!(group_entries(&[base, explicit], true).len(), 2);
    }

    #[test]
    fn same_provider_alternatives_and_duration_chains_do_not_overmerge() {
        let items = vec![
            song(
                Provider::YouTubeMusic,
                "yt-a",
                "Song",
                &["Artist"],
                Some(200),
            ),
            song(
                Provider::YouTubeMusic,
                "yt-b",
                "Song",
                &["Artist"],
                Some(201),
            ),
            song(Provider::Spotify, "sp", "Song", &["Artist"], Some(200)),
        ];
        assert_eq!(group_entries(&items, true).len(), 3);
        let chain = vec![
            song(Provider::YouTubeMusic, "yt", "Song", &["Artist"], Some(200)),
            song(Provider::Spotify, "sp", "Song", &["Artist"], Some(203)),
            song(Provider::Local, "file", "Song", &["Artist"], Some(206)),
        ];
        let groups = group_entries(&chain, true);
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|group| group.variants.len() <= 2));
    }

    #[test]
    fn stable_keys_and_membership_survive_ranking_changes() {
        let first = song(Provider::YouTubeMusic, "yt", "Song", &["Artist"], Some(200));
        let second = song(Provider::Spotify, "sp", "Song", &["Artist"], Some(201));
        let before = group_entries(&[first.clone(), second.clone()], true);
        let after = group_entries(&[second, first], true);
        assert_eq!(before[0].key, after[0].key);
        assert_eq!(
            before[0].preferred_entry(Provider::Spotify, None),
            after[0].preferred_entry(Provider::Spotify, None)
        );
    }

    #[test]
    fn videos_albums_and_playlists_stay_separate() {
        let song = song(Provider::YouTubeMusic, "yt", "Song", &["Artist"], Some(200));
        let video = Entry::Video(song.track().unwrap().clone());
        let album = Entry::Album(AlbumRef {
            id: "album".to_owned(),
            title: "Song".to_owned(),
            artist: "Artist".to_owned(),
            artists: vec![ArtistLink::plain("Artist")],
            explicit: None,
            year: None,
            kind: None,
            thumbnail: None,
        });
        let playlist = Entry::Playlist(crate::model::PlaylistRef {
            id: "playlist".to_owned(),
            title: "Song".to_owned(),
            author: "Artist".to_owned(),
            author_id: None,
            track_count: None,
            thumbnail: None,
            editable: false,
            source: PlaylistSource::YouTube,
        });
        assert_eq!(
            group_entries(&[song, video, album, playlist], true).len(),
            4
        );
    }

    #[test]
    fn artist_names_require_unique_source_candidates() {
        let youtube = artist(Provider::YouTubeMusic, "yt", "  Artist Name ");
        let spotify = artist(Provider::Spotify, "sp", "artist name");
        let groups = group_entries(&[youtube.clone(), spotify.clone()], true);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].evidence, MatchEvidence::ArtistName);
        let other = artist(Provider::YouTubeMusic, "different-yt", "Artist Name");
        assert_eq!(group_entries(&[youtube, spotify, other], true).len(), 3);
    }

    #[test]
    fn matched_song_credits_strengthen_and_constrain_artist_links() {
        let mut youtube_song = song(
            Provider::YouTubeMusic,
            "yt-song",
            "Song",
            &["Artist"],
            Some(200),
        );
        youtube_song.track_mut().unwrap().artists[0].id = Some("yt-artist".to_owned());
        let mut spotify_song = song(Provider::Spotify, "sp-song", "Song", &["Artist"], Some(201));
        spotify_song.track_mut().unwrap().artists[0].id =
            Some("spotify:artist:sp-artist".to_owned());
        let youtube_artist = artist(Provider::YouTubeMusic, "yt-artist", "Artist");
        let spotify_artist = artist(Provider::Spotify, "sp-artist", "Artist");
        let groups = group_entries(
            &[
                youtube_song.clone(),
                spotify_song.clone(),
                youtube_artist.clone(),
                spotify_artist,
            ],
            true,
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].evidence, MatchEvidence::ArtistSongCredits);
        let unrelated = artist(Provider::Spotify, "unrelated", "Artist");
        let groups = group_entries(
            &[youtube_song, spotify_song, youtube_artist, unrelated],
            true,
        );
        assert_eq!(groups.len(), 3);
        assert!(groups[1..].iter().all(|group| !group.is_combined()));
    }

    #[test]
    fn unavailable_override_uses_default_then_first_variant() {
        let items = vec![
            song(Provider::YouTubeMusic, "yt", "Song", &["Artist"], Some(200)),
            song(Provider::Spotify, "sp", "Song", &["Artist"], Some(201)),
        ];
        let groups = group_entries(&items, true);
        assert_eq!(
            groups[0].preferred_index(Provider::Spotify, Some(Provider::Local)),
            Some(1)
        );
        assert_eq!(groups[0].preferred_index(Provider::Local, None), Some(0));
        assert_eq!(
            groups[0].providers(),
            [Provider::YouTubeMusic, Provider::Spotify]
        );
    }

    #[test]
    fn paired_song_credits_open_profiles_without_an_artist_card() {
        let youtube_id = "UCartist";
        let spotify_id = "spotify:artist:74ASZWbe4lXaubB36ztrGX";
        let mut youtube = song(
            Provider::YouTubeMusic,
            "yt-song",
            "Song",
            &["Artist"],
            Some(200),
        );
        let mut spotify = song(Provider::Spotify, "sp-song", "Song", &["ARTIST"], Some(201));
        youtube.track_mut().unwrap().artists[0].id = Some(youtube_id.to_owned());
        spotify.track_mut().unwrap().artists[0].id = Some(spotify_id.to_owned());
        let groups = group_entries(&[youtube, spotify], true);
        assert_eq!(groups.len(), 1);
        let pair = Some((youtube_id.to_owned(), spotify_id.to_owned()));
        assert_eq!(groups[0].artist_profiles(youtube_id), pair);
        assert_eq!(groups[0].artist_profiles(spotify_id), pair);
        assert!(groups[0].artist_profiles("unrelated").is_none());
    }

    #[test]
    fn duration_fallback_does_not_strengthen_artist_profile_links() {
        let youtube_id = "UCartist";
        let spotify_id = "spotify:artist:74ASZWbe4lXaubB36ztrGX";
        for duration in [None, Some(0)] {
            let mut youtube = song(
                Provider::YouTubeMusic,
                "yt-song",
                "Song",
                &["Artist"],
                duration,
            );
            let mut spotify = song(Provider::Spotify, "sp-song", "Song", &["Artist"], Some(200));
            youtube.track_mut().unwrap().artists[0].id = Some(youtube_id.to_owned());
            spotify.track_mut().unwrap().artists[0].id = Some(spotify_id.to_owned());
            let groups = group_entries(
                &[
                    youtube,
                    spotify,
                    artist(Provider::YouTubeMusic, youtube_id, "Artist"),
                    artist(Provider::Spotify, "74ASZWbe4lXaubB36ztrGX", "Artist"),
                ],
                true,
            );
            assert_eq!(groups.len(), 2);
            assert_eq!(groups[0].variants.len(), 2);
            assert!(groups[0].artist_profiles(youtube_id).is_none());
            assert!(groups[0].artist_profiles(spotify_id).is_none());
            assert_eq!(groups[1].evidence, MatchEvidence::ArtistName);
        }
    }

    #[test]
    fn duplicate_or_incomplete_song_credits_do_not_choose_a_profile_pair() {
        let mut youtube = song(
            Provider::YouTubeMusic,
            "yt-song",
            "Song",
            &["Artist"],
            Some(200),
        );
        let mut spotify = song(Provider::Spotify, "sp-song", "Song", &["Artist"], Some(201));
        youtube.track_mut().unwrap().artists[0].id = Some("UCartist".to_owned());
        spotify.track_mut().unwrap().artists[0].id =
            Some("spotify:artist:74ASZWbe4lXaubB36ztrGX".to_owned());
        let duplicate = spotify.track().unwrap().artists[0].clone();
        spotify.track_mut().unwrap().artists.push(duplicate);
        let groups = group_entries(&[youtube.clone(), spotify.clone()], true);
        assert!(groups[0].artist_profiles("UCartist").is_none());
        spotify.track_mut().unwrap().artists.pop();
        spotify.track_mut().unwrap().artists[0].id = None;
        let groups = group_entries(&[youtube, spotify], true);
        assert!(groups[0].artist_profiles("UCartist").is_none());
    }

    #[test]
    fn artist_groups_return_only_their_native_remote_ids() {
        let youtube = artist(Provider::YouTubeMusic, "UCartist", "Artist");
        let spotify = artist(Provider::Spotify, "74ASZWbe4lXaubB36ztrGX", "Artist");
        let groups = group_entries(&[youtube, spotify], true);
        assert_eq!(
            groups[0].artist_profiles("UCartist"),
            Some((
                "UCartist".to_owned(),
                "spotify:artist:74ASZWbe4lXaubB36ztrGX".to_owned()
            ))
        );
        assert!(groups[0].artist_profiles("other").is_none());
    }
}
