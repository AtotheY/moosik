//! Personal song ranks derived from lifetime completed listens.
//!
//! A completion is an ended session with at least 90% distinct track coverage;
//! the collection ledger counts those sessions and supplies that count here.
use super::collection::{Collection, Sleeve};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SongRank {
    Iron,
    Bronze,
    Silver,
    Gold,
    Platinum,
    Diamond,
    DoublePlatinum,
}

impl SongRank {
    pub const ALL: [Self; 7] = [
        Self::Iron,
        Self::Bronze,
        Self::Silver,
        Self::Gold,
        Self::Platinum,
        Self::Diamond,
        Self::DoublePlatinum,
    ];

    pub fn from_completed(completed: u64) -> Option<Self> {
        Self::ALL
            .into_iter()
            .rev()
            .find(|rank| completed >= rank.threshold())
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Iron => "Iron",
            Self::Bronze => "Bronze",
            Self::Silver => "Silver",
            Self::Gold => "Gold",
            Self::Platinum => "Platinum",
            Self::Diamond => "Diamond",
            Self::DoublePlatinum => "Double Platinum",
        }
    }

    pub const fn threshold(self) -> u64 {
        match self {
            Self::Iron => 10,
            Self::Bronze => 25,
            Self::Silver => 50,
            Self::Gold => 100,
            Self::Platinum => 250,
            Self::Diamond => 500,
            Self::DoublePlatinum => 1_000,
        }
    }

    pub const fn index(self) -> usize {
        self as usize
    }

    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }

    /// The next rank for a song, including Iron when it has no earned rank yet.
    pub fn next_goal(completed: u64) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|rank| completed < rank.threshold())
    }
}

fn song_order(a_id: &str, a: &Sleeve, b_id: &str, b: &Sleeve) -> Ordering {
    b.completed
        .cmp(&a.completed)
        .then_with(|| b.listen_ms.total_cmp(&a.listen_ms))
        .then_with(|| a_id.cmp(b_id))
}

/// Lifetime song order: completed listens, then total listening time, then stable track ID.
pub fn ranked_song_ids(collection: &Collection) -> Vec<&str> {
    let mut songs: Vec<_> = collection.sleeves.iter().collect();
    songs.sort_by(|(a_id, a), (b_id, b)| song_order(a_id, a, b_id, b));
    songs.into_iter().map(|(id, _)| id.as_str()).collect()
}

/// One-based position without sorting the whole collection on every player frame.
pub fn song_position(collection: &Collection, track_id: &str) -> Option<usize> {
    let song = collection.sleeves.get(track_id)?;
    Some(
        1 + collection
            .sleeves
            .iter()
            .filter(|(id, other)| song_order(id, other, track_id, song).is_lt())
            .count(),
    )
}

#[cfg(test)]
mod tests {
    use super::{ranked_song_ids, song_position};
    use crate::listening::collection::{Collection, Sleeve};

    #[test]
    fn shelf_order_and_player_position_agree_on_ties() {
        let mut collection = Collection::default();
        for (id, completed, listen_ms) in [
            ("later-id", 25, 90_000.),
            ("first-id", 25, 90_000.),
            ("more-time", 25, 110_000.),
            ("more-full", 50, 50_000.),
        ] {
            collection.sleeves.insert(
                id.into(),
                Sleeve {
                    completed,
                    listen_ms,
                    ..Default::default()
                },
            );
        }
        let ordered = ranked_song_ids(&collection);
        assert_eq!(ordered, ["more-full", "more-time", "first-id", "later-id"]);
        for (index, id) in ordered.into_iter().enumerate() {
            assert_eq!(song_position(&collection, id), Some(index + 1));
        }
    }
}
