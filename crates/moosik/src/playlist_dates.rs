//! YouTube does not return historical addition dates on browse. Remember confirmed additions
//! by playlist item ID, so two copies of the same recording never borrow each other's date.
use std::collections::BTreeMap;

use crate::model::{Track, bare_playlist_id};

#[derive(Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct RecordedDates(BTreeMap<String, BTreeMap<String, i64>>);

impl RecordedDates {
    fn path() -> Option<std::path::PathBuf> {
        Some(
            crate::profile_dirs()?
                .config_dir()
                .join("playlist_dates.json"),
        )
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Some(path) = Self::path()
            && let Err(e) = (|| -> anyhow::Result<()> {
                std::fs::create_dir_all(path.parent().unwrap())?;
                crate::persist::write_json_atomic(&path, &serde_json::to_vec(self)?)?;
                Ok(())
            })()
        {
            tracing::warn!(error = %e, "could not save playlist addition dates");
        }
    }

    pub fn record(&mut self, id: &str, entries: Vec<(String, i64)>) {
        if !entries.is_empty() {
            self.0
                .entry(bare_playlist_id(id).to_owned())
                .or_default()
                .extend(entries);
            self.save();
        }
    }

    pub fn forget(&mut self, id: &str, item: Option<&str>) {
        let id = bare_playlist_id(id);
        let changed = match item {
            Some(key) => self.0.get_mut(id).and_then(|m| m.remove(key)).is_some(),
            None => self.0.remove(id).is_some(),
        };
        if changed {
            self.save();
        }
    }

    pub fn apply(&self, id: &str, tracks: &mut [Track]) {
        if let Some(dates) = self.0.get(bare_playlist_id(id)) {
            for track in tracks {
                if let Some(date) = track.set_video_id.as_deref().and_then(|key| dates.get(key)) {
                    track.playlist_added_ms = Some(*date);
                }
            }
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AddedFilter {
    #[default]
    Any,
    Week,
    Month,
}

impl AddedFilter {
    pub const ALL: [Self; 3] = [Self::Any, Self::Week, Self::Month];
    pub fn label(self) -> &'static str {
        match self {
            Self::Any => "Any time",
            Self::Week => "Last 7 days",
            Self::Month => "Last 30 days",
        }
    }

    pub fn matches(self, added: Option<i64>, now: i64) -> bool {
        let days = match self {
            Self::Any => return true,
            Self::Week => 7,
            Self::Month => 30,
        };
        added.is_some_and(|date| (now - days * 86_400_000..=now).contains(&date))
    }
}

/// Keep the matching rows for painting and playback without deep-cloning every visible song
/// on every frame. Compare against the live entries so row edits and rolling cutoffs stay current.
#[derive(Default)]
pub(crate) struct FilteredTracks(Option<std::sync::Arc<[Track]>>);
impl FilteredTracks {
    pub fn project(
        &mut self,
        filter: AddedFilter,
        tracks: &crate::model::Paged<Track>,
        now: i64,
    ) -> Option<std::sync::Arc<[Track]>> {
        if filter == AddedFilter::Any {
            self.0 = None;
            return None;
        }
        // No partial counts or queues while waiting for the last continuation.
        let complete = if tracks.next.is_none() {
            tracks.items.as_slice()
        } else {
            &[]
        };
        let matches = complete
            .iter()
            .filter(|t| filter.matches(t.playlist_added_ms, now));
        if self
            .0
            .as_ref()
            .is_none_or(|cached| !cached.iter().eq(matches.clone()))
        {
            self.0 = Some(matches.cloned().collect());
        }
        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_dates_do_not_cross_between_lists_or_duplicate_items() {
        let store: RecordedDates =
            serde_json::from_str(r#"{"PLone":{"entry-A":18,"entry-B":20},"PLtwo":{"entry-A":42}}"#)
                .unwrap();
        let make = |key: &str| {
            let mut t = crate::listening::Metadata {
                id: "same-song".into(),
                ..Default::default()
            }
            .to_track();
            t.set_video_id = Some(key.into());
            t
        };
        let mut tracks = [make("entry-B"), make("entry-A"), make("new-copy")];
        store.apply("VLPLone", &mut tracks);
        assert_eq!(
            tracks.map(|t| t.playlist_added_ms),
            [Some(20), Some(18), None]
        );
        let mut other = [make("entry-A")];
        store.apply("PLtwo", &mut other);
        assert_eq!(other[0].playlist_added_ms, Some(42));
    }

    #[test]
    fn playlist_dates_filter_waits_for_the_last_page_and_reflects_edits() {
        let mut first = crate::listening::Metadata {
            id: "one".into(),
            ..Default::default()
        }
        .to_track();
        first.playlist_added_ms = Some(123);
        let mut page = crate::model::Paged::new(vec![first.clone()], Some("next".into()));
        let mut view = FilteredTracks::default();
        assert!(
            view.project(AddedFilter::Week, &page, 124)
                .unwrap()
                .is_empty()
        );
        let mut second = first.clone();
        second.video_id = "two".into();
        page.extend(vec![second.clone()], None);
        assert_eq!(
            &*view.project(AddedFilter::Week, &page, 124).unwrap(),
            &[first.clone(), second.clone()]
        );
        page.items.remove(0);
        assert_eq!(
            &*view.project(AddedFilter::Week, &page, 124).unwrap(),
            &[second]
        );
        assert!(view.project(AddedFilter::Any, &page, 124).is_none());
    }

    #[test]
    fn playlist_dates_filter_excludes_unknowns_and_includes_boundary() {
        let day = 86_400_000;
        let now = 50 * day;
        assert!(AddedFilter::Any.matches(None, now));
        for (filter, days) in [(AddedFilter::Week, 7), (AddedFilter::Month, 30)] {
            assert!(filter.matches(Some(now - days * day), now));
            assert!(!filter.matches(Some(now - days * day - 1), now));
            assert!(!filter.matches(None, now));
            assert!(!filter.matches(Some(now + 1), now));
        }
    }
}
