use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use tokio::sync::OnceCell;

use crate::model::{Provider, SearchResults, SearchScope};

const TTL: Duration = Duration::from_secs(10 * 60);
const CAPACITY: usize = 32;
const ITEM_BUDGET: usize = 2048;
const ENTRY_ITEM_BUDGET: usize = 512;
const FLIGHT_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    generation: u64,
    local_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Results,
    Suggestions,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    provider: Provider,
    query: String,
    scope: SearchScope,
    generation: u64,
    local_revision: u64,
    kind: Kind,
}

impl Key {
    pub(crate) fn results(
        provider: Provider,
        query: &str,
        scope: SearchScope,
        snapshot: Snapshot,
    ) -> Self {
        Self {
            provider,
            query: query.trim().to_owned(),
            scope: if provider == Provider::Local {
                SearchScope::All
            } else {
                scope
            },
            generation: snapshot.generation,
            local_revision: if provider == Provider::Local {
                snapshot.local_revision
            } else {
                0
            },
            kind: Kind::Results,
        }
    }

    pub(crate) fn suggestions(provider: Provider, query: &str, snapshot: Snapshot) -> Self {
        Self {
            kind: Kind::Suggestions,
            ..Self::results(provider, query, SearchScope::All, snapshot)
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Value {
    Results(SearchResults),
    Suggestions(Vec<String>),
}

impl Value {
    fn items(&self) -> usize {
        match self {
            Self::Results(results) => results.items.len().max(1),
            Self::Suggestions(items) => items.len().max(1),
        }
    }
}

#[derive(Clone)]
struct Fetched {
    value: Arc<Value>,
    expires_at: Instant,
}

type SharedResult = Result<Fetched, Arc<str>>;

struct Entry {
    fetched: Fetched,
    last_used: u64,
}

#[derive(Default)]
struct State {
    generation: u64,
    local_revision: u64,
    entries: HashMap<Key, Entry>,
    flights: HashMap<Key, Weak<OnceCell<SharedResult>>>,
    items: usize,
    sequence: u64,
}

impl State {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            generation: self.generation,
            local_revision: self.local_revision,
        }
    }

    fn accepts(&self, key: &Key) -> bool {
        key.generation == self.generation
            && (key.provider != Provider::Local || key.local_revision == self.local_revision)
    }

    fn prune(&mut self, now: Instant) {
        self.entries
            .retain(|_, entry| entry.fetched.expires_at > now);
        self.flights.retain(|_, flight| flight.strong_count() > 0);
        self.items = self
            .entries
            .values()
            .map(|entry| entry.fetched.value.items())
            .sum();
    }
}

pub(crate) struct SearchCache {
    state: Mutex<State>,
    ttl: Duration,
    capacity: usize,
    item_budget: usize,
    entry_item_budget: usize,
}

impl Default for SearchCache {
    fn default() -> Self {
        Self::with_limits(TTL, CAPACITY, ITEM_BUDGET, ENTRY_ITEM_BUDGET)
    }
}

impl SearchCache {
    fn with_limits(
        ttl: Duration,
        capacity: usize,
        item_budget: usize,
        entry_item_budget: usize,
    ) -> Self {
        Self {
            state: Mutex::new(State::default()),
            ttl,
            capacity,
            item_budget,
            entry_item_budget,
        }
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .snapshot()
    }

    pub(crate) fn invalidate(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.generation = state.generation.wrapping_add(1);
        state.entries.clear();
        state.flights.clear();
        state.items = 0;
    }

    pub(crate) fn invalidate_local(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.local_revision = state.local_revision.wrapping_add(1);
        state
            .entries
            .retain(|key, _| key.provider != Provider::Local);
        state
            .flights
            .retain(|key, _| key.provider != Provider::Local);
        state.items = state
            .entries
            .values()
            .map(|entry| entry.fetched.value.items())
            .sum();
    }

    pub(crate) async fn get_or_fetch<F, Fut>(
        &self,
        key: Key,
        fetch: F,
    ) -> anyhow::Result<Arc<Value>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<Value>>,
    {
        let cell = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.prune(Instant::now());
            state.sequence = state.sequence.wrapping_add(1);
            let sequence = state.sequence;
            if let Some(entry) = state.entries.get_mut(&key) {
                entry.last_used = sequence;
                return Ok(Arc::clone(&entry.fetched.value));
            }
            let shared = state
                .flights
                .get(&key)
                .and_then(Weak::upgrade)
                .filter(|cell| {
                    cell.get().is_none_or(|result| {
                        result
                            .as_ref()
                            .is_ok_and(|fetched| fetched.expires_at > Instant::now())
                            || result.is_err()
                    })
                });
            match shared {
                Some(cell) => cell,
                None => {
                    let cell = Arc::new(OnceCell::new());
                    if state.accepts(&key) && state.flights.len() < FLIGHT_CAPACITY {
                        state.flights.insert(key.clone(), Arc::downgrade(&cell));
                    }
                    cell
                }
            }
        };
        let result = cell
            .get_or_init(|| async {
                fetch()
                    .await
                    .map(|value| Fetched {
                        value: Arc::new(value),
                        expires_at: Instant::now() + self.ttl,
                    })
                    .map_err(|error| Arc::<str>::from(format!("{error:#}")))
            })
            .await;
        match result {
            Ok(fetched) => {
                self.store(key, fetched.clone());
                Ok(Arc::clone(&fetched.value))
            }
            Err(error) => Err(anyhow::anyhow!(error.to_string())),
        }
    }

    fn store(&self, key: Key, fetched: Fetched) {
        let items = fetched.value.items();
        if key.query.len() > 1024
            || items > self.entry_item_budget
            || items > self.item_budget
            || self.capacity == 0
        {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.prune(Instant::now());
        if !state.accepts(&key) || fetched.expires_at <= Instant::now() {
            return;
        }
        if state.entries.contains_key(&key) {
            return;
        }
        while state.entries.len() >= self.capacity || state.items + items > self.item_budget {
            let oldest = state
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else {
                break;
            };
            if let Some(entry) = state.entries.remove(&oldest) {
                state.items = state.items.saturating_sub(entry.fetched.value.items());
            }
        }
        state.sequence = state.sequence.wrapping_add(1);
        let last_used = state.sequence;
        state.items += items;
        state.entries.insert(key, Entry { fetched, last_used });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn key(cache: &SearchCache, query: &str) -> Key {
        Key::suggestions(Provider::YouTubeMusic, query, cache.snapshot())
    }

    async fn fetch(cache: &SearchCache, key: Key, calls: &AtomicUsize, count: usize) -> Arc<Value> {
        cache
            .get_or_fetch(key, || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(Value::Suggestions(
                    (0..count).map(|index| format!("item-{index}")).collect(),
                ))
            })
            .await
            .unwrap()
    }

    fn count(value: &Value) -> usize {
        match value {
            Value::Suggestions(items) => items.len(),
            Value::Results(results) => results.items.len(),
        }
    }

    #[tokio::test]
    async fn successful_values_expire_and_are_fetched_again() {
        let cache = SearchCache::default();
        let calls = AtomicUsize::new(0);
        let key = key(&cache, "song");
        fetch(&cache, key.clone(), &calls, 2).await;
        fetch(&cache, key.clone(), &calls, 2).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cache
            .state
            .lock()
            .unwrap()
            .entries
            .get_mut(&key)
            .unwrap()
            .fetched
            .expires_at = Instant::now();
        fetch(&cache, key, &calls, 2).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn errors_do_not_replace_a_later_successful_response() {
        let cache = SearchCache::default();
        let calls = AtomicUsize::new(0);
        let key = key(&cache, "retry");
        let result = cache
            .get_or_fetch(key.clone(), || async {
                calls.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("temporary network failure")
            })
            .await;
        assert!(result.is_err());
        fetch(&cache, key.clone(), &calls, 1).await;
        fetch(&cache, key, &calls, 1).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn capacity_and_item_budget_keep_recent_entries() {
        let cache = SearchCache::with_limits(TTL, 2, 3, 3);
        let calls = AtomicUsize::new(0);
        let first = key(&cache, "first");
        let second = key(&cache, "second");
        let third = key(&cache, "third");
        fetch(&cache, first.clone(), &calls, 2).await;
        fetch(&cache, second.clone(), &calls, 1).await;
        fetch(&cache, first.clone(), &calls, 2).await;
        fetch(&cache, third, &calls, 1).await;
        fetch(&cache, first, &calls, 2).await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        fetch(&cache, second, &calls, 1).await;
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        let state = cache.state.lock().unwrap();
        assert!(state.entries.len() <= 2);
        assert!(state.items <= 3);
    }

    #[tokio::test]
    async fn oversized_responses_stay_complete_and_are_not_cached() {
        let cache = SearchCache::with_limits(TTL, 32, 10, 3);
        let calls = AtomicUsize::new(0);
        let key = key(&cache, "large");
        assert_eq!(
            count(fetch(&cache, key.clone(), &calls, 4).await.as_ref()),
            4
        );
        assert_eq!(count(fetch(&cache, key, &calls, 4).await.as_ref()), 4);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(cache.state.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn keys_trim_queries_and_keep_source_scope_and_uri_case() {
        let cache = SearchCache::default();
        let snapshot = cache.snapshot();
        let youtube = Key::results(
            Provider::YouTubeMusic,
            "  A song  ",
            SearchScope::All,
            snapshot,
        );
        assert_eq!(
            youtube,
            Key::results(Provider::YouTubeMusic, "A song", SearchScope::All, snapshot)
        );
        assert_ne!(
            youtube,
            Key::results(Provider::Spotify, "A song", SearchScope::All, snapshot)
        );
        assert_ne!(
            youtube,
            Key::results(
                Provider::YouTubeMusic,
                "A song",
                SearchScope::Songs,
                snapshot
            )
        );
        assert_ne!(
            key(&cache, "spotify:track:3TO7bbrUKrOSPGRTB5MeCz"),
            key(&cache, "spotify:track:3to7bbrukrospgrtb5mecz")
        );
        assert_eq!(
            Key::results(Provider::Local, "song", SearchScope::All, snapshot),
            Key::results(Provider::Local, "song", SearchScope::Songs, snapshot)
        );
    }

    #[tokio::test]
    async fn concurrent_identical_requests_share_one_fetch() {
        let cache = Arc::new(SearchCache::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(tokio::sync::Barrier::new(8));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                cache
                    .get_or_fetch(key(&cache, "shared"), || async {
                        calls.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        Ok(Value::Suggestions(vec!["result".to_owned()]))
                    })
                    .await
                    .unwrap()
            }));
        }
        for task in tasks {
            assert_eq!(count(&task.await.unwrap()), 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn account_invalidation_rejects_old_inflight_insertions() {
        let cache = Arc::new(SearchCache::default());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let task = {
            let cache = Arc::clone(&cache);
            tokio::spawn(async move {
                cache
                    .get_or_fetch(key(&cache, "old account"), || async {
                        started_tx.send(()).unwrap();
                        release_rx.await.unwrap();
                        Ok(Value::Suggestions(vec!["old account result".to_owned()]))
                    })
                    .await
                    .unwrap()
            })
        };
        started_rx.await.unwrap();
        cache.invalidate();
        release_tx.send(()).unwrap();
        task.await.unwrap();
        assert!(cache.state.lock().unwrap().entries.is_empty());
        let calls = AtomicUsize::new(0);
        fetch(&cache, key(&cache, "old account"), &calls, 1).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn local_changes_preserve_remote_cache_and_replace_local_keys() {
        let cache = SearchCache::default();
        let calls = AtomicUsize::new(0);
        let remote = key(&cache, "song");
        let local = Key::suggestions(Provider::Local, "song", cache.snapshot());
        fetch(&cache, remote.clone(), &calls, 1).await;
        fetch(&cache, local.clone(), &calls, 1).await;
        cache.invalidate_local();
        fetch(&cache, remote, &calls, 1).await;
        let updated = Key::suggestions(Provider::Local, "song", cache.snapshot());
        assert_ne!(local, updated);
        fetch(&cache, updated, &calls, 1).await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(!cache.state.lock().unwrap().entries.contains_key(&local));
    }
}
