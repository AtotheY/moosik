//! Recent searches: what was typed into the search box, newest first.
//!
//! Kept on this machine only, beside the other local stores in the config dir, and offered by the
//! search box when it is focused and empty (`ui/topbar.rs`). Nothing here is sent anywhere, so it
//! works the same signed out.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// How many searches are kept; the dropdown scrolls from the sixth on.
pub const MAX: usize = 15;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHistory {
    pub entries: Vec<String>,
}

impl SearchHistory {
    /// Put `query` at the top. A repeat moves up rather than appearing twice, matched without
    /// regard to case and kept in its newest spelling; blank text is not a search.
    pub fn record(&mut self, query: &str) {
        let query = query.trim();
        if query.is_empty() {
            return;
        }
        let folded = query.to_lowercase();
        self.entries.retain(|e| e.to_lowercase() != folded);
        self.entries.insert(0, query.to_owned());
        self.entries.truncate(MAX);
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.remove(index);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

fn path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "refryt")
        .context("could not find the config directory")?;
    Ok(dirs.config_dir().join("search_history.json"))
}

/// Load the saved searches, empty for a missing or unreadable file: a broken file should not stop
/// the app starting, same reasoning as `settings::load`.
pub fn load() -> SearchHistory {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

pub fn save(history: &SearchHistory) -> Result<()> {
    save_to(history, &path()?)
}

fn load_from(path: &Path) -> SearchHistory {
    let Ok(bytes) = std::fs::read(path) else {
        return SearchHistory::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "search history could not be parsed; starting empty");
        SearchHistory::default()
    })
}

fn save_to(history: &SearchHistory, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(history).context("could not encode search history")?;
    crate::persist::write_json_atomic(path, &json)
        .with_context(|| format!("could not write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeat_moves_to_the_top_in_its_newest_spelling() {
        let mut h = SearchHistory::default();
        h.record("travis scott");
        h.record("mary j blige");
        h.record("  Travis Scott ");
        assert_eq!(h.entries, ["Travis Scott", "mary j blige"]);
    }

    #[test]
    fn blank_searches_are_not_kept() {
        let mut h = SearchHistory::default();
        h.record("");
        h.record("   ");
        assert!(h.entries.is_empty());
    }

    #[test]
    fn only_the_newest_fifteen_are_kept() {
        let mut h = SearchHistory::default();
        for i in 0..20 {
            h.record(&format!("search {i}"));
        }
        assert_eq!(h.entries.len(), MAX);
        assert_eq!(h.entries[0], "search 19");
        assert_eq!(h.entries[MAX - 1], "search 5");
    }

    #[test]
    fn remove_and_clear() {
        let mut h = SearchHistory::default();
        h.record("a");
        h.record("b");
        h.remove(0);
        assert_eq!(h.entries, ["a"]);
        h.remove(5);
        assert_eq!(h.entries, ["a"]);
        h.clear();
        assert!(h.entries.is_empty());
    }

    #[test]
    fn it_survives_a_save_and_load_and_a_broken_file_reads_as_empty() {
        let dir =
            std::env::temp_dir().join(format!("refryt-search-history-{}", std::process::id()));
        let path = dir.join("search_history.json");
        let mut h = SearchHistory::default();
        h.record("verdi requiem");
        save_to(&h, &path).unwrap();
        assert_eq!(load_from(&path), h);
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(load_from(&path), SearchHistory::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
