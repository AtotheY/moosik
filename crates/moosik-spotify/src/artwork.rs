use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, LazyLock, Mutex},
};

use serde_json::Value;
use url::Url;

const MAX_ARTWORK: usize = 1024;
const MAX_VARIANTS: usize = 4;
const MAX_URL_BYTES: usize = 256;

static ARTWORK: LazyLock<Mutex<ArtworkCache>> =
    LazyLock::new(|| Mutex::new(ArtworkCache::default()));

#[derive(Default)]
struct ArtworkCache {
    sources: HashMap<Arc<str>, Vec<(u64, Arc<str>)>>,
    order: VecDeque<Arc<str>>,
}

impl ArtworkCache {
    fn remember(&mut self, original: &str, variants: Vec<(u64, Arc<str>)>) {
        if let Some(sources) = self.sources.get_mut(original) {
            *sources = variants;
            return;
        }
        if self.sources.len() == MAX_ARTWORK
            && let Some(oldest) = self.order.pop_front()
        {
            self.sources.remove(&oldest);
        }
        let original: Arc<str> = original.into();
        self.order.push_back(Arc::clone(&original));
        self.sources.insert(original, variants);
    }
}

pub(crate) fn image(value: &Value) -> Option<String> {
    let mut sources: Vec<_> = value
        .as_array()?
        .iter()
        .filter_map(|item| {
            let url = Url::parse(item["url"].as_str()?).ok()?;
            (url.scheme() == "https" && url.username().is_empty() && url.password().is_none())
                .then(|| (item["width"].as_u64().unwrap_or(0), url))
        })
        .collect();
    sources.sort_by_key(|(width, _)| *width);
    let (width, original) = sources.pop()?;
    let original_url = original.as_str();
    if original_url.len() <= MAX_URL_BYTES {
        let variants: Vec<_> = sources
            .into_iter()
            .filter(|(size, url)| {
                *size > 0
                    && *size < width
                    && url.as_str().len() <= MAX_URL_BYTES
                    && url.as_str() != original_url
                    && url.origin() == original.origin()
            })
            .take(MAX_VARIANTS)
            .map(|(width, url)| (width, Arc::from(url.as_str())))
            .collect();
        if !variants.is_empty()
            && let Ok(mut cache) = ARTWORK.lock()
        {
            cache.remember(original_url, variants);
        }
    }
    Some(original.into())
}

pub fn sized_artwork_url(original: &str, width: u32) -> Option<Arc<str>> {
    ARTWORK
        .lock()
        .ok()?
        .sources
        .get(original)?
        .iter()
        .find(|(size, _)| *size >= u64::from(width))
        .map(|(_, url)| Arc::clone(url))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn selects_the_smallest_supplied_cover_that_fits() {
        let original = "https://i.scdn.co/image/artwork-test-large";
        let medium = "https://i.scdn.co/image/artwork-test-medium";
        let small = "https://i.scdn.co/image/artwork-test-small";
        let sources = json!([
            { "url": medium, "width": 300 },
            { "url": original, "width": 640 },
            { "url": "https://other.example/cover", "width": 128 },
            { "url": "https://user:pass@i.scdn.co/cover", "width": 1024 },
            { "url": small, "width": 64 }
        ]);
        assert_eq!(image(&sources).as_deref(), Some(original));
        assert_eq!(sized_artwork_url(original, 64).as_deref(), Some(small));
        assert_eq!(sized_artwork_url(original, 128).as_deref(), Some(medium));
        assert_eq!(sized_artwork_url(original, 512), None);
        assert_eq!(
            sized_artwork_url("https://i.scdn.co/image/unknown", 64),
            None
        );
    }

    #[test]
    fn evicts_old_artwork_at_capacity() {
        let mut cache = ArtworkCache::default();
        for index in 0..=MAX_ARTWORK {
            cache.remember(&index.to_string(), vec![(64, "small".into())]);
        }
        assert_eq!(cache.sources.len(), MAX_ARTWORK);
        assert_eq!(cache.order.len(), MAX_ARTWORK);
        assert!(!cache.sources.contains_key("0"));
        assert!(cache.sources.contains_key(MAX_ARTWORK.to_string().as_str()));
    }
}
