use super::*;
use crate::discord::SessionProfile;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Member {
    pub id: u64,
    pub name: String,
    #[serde(default, rename = "avatar", deserialize_with = "read_avatar")]
    pub photo: Option<Arc<str>>,
}

fn read_avatar<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<Option<Arc<str>>, D::Error> {
    let key = Option::<String>::deserialize(reader)?;
    Ok(key.as_deref().and_then(crate::discord::avatar_url))
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Entry {
    pub entry: u64,
    pub song: Song,
    pub by: Member,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Snapshot {
    pub members: Vec<Member>,
    pub queue: Vec<Entry>,
    pub you: u64,
    #[serde(default)]
    pub(super) can_pick_entry: bool,
    pub current_by: Option<Member>,
    pub(super) state: Option<Song>,
    pub(super) error: Option<String>,
    #[serde(skip)]
    pub(super) added: bool,
}

impl Snapshot {
    pub(crate) fn playing_credit(&self, id: &str) -> Option<(&Member, String)> {
        if self.state.as_ref()?.id != id {
            return None;
        }
        if let Some(member) = &self.current_by {
            Some((member, format!("Added by {}", member.name)))
        } else {
            let host = self.members.iter().find(|m| m.id == 0)?;
            Some((host, format!("From {}’s queue", host.name)))
        }
    }
}

#[derive(Clone, Serialize)]
struct Addition {
    seq: u64,
    song: Song,
}

// A host may make several playback choices between heartbeats. Keep the last choice and
// the remaining entries below the high-water mark so retries describe the same edit.
#[derive(Clone, Default, PartialEq)]
struct Played {
    through: u64,
    keep_entries: Vec<u64>,
    play_entry: Option<u64>,
}

impl Played {
    fn consume(&mut self, entry: u64, remaining: &[Entry]) {
        self.through = self.through.max(entry);
        self.keep_entries = remaining
            .iter()
            .map(|e| e.entry)
            .take_while(|&id| id <= self.through)
            .collect();
        self.play_entry = (entry != self.through).then_some(entry);
    }

    fn keep(&self, entry: u64) -> bool {
        entry > self.through || self.keep_entries.contains(&entry)
    }
}

#[derive(Default)]
struct Commands {
    room: String,
    profile: SessionProfile,
    sequence: u64,
    pending: VecDeque<Addition>,
    played: Played,
    sent_played: Played,
    sent_profile: SessionProfile,
}

/// UI writes into a room-scoped outbox. Each heartbeat retries its oldest unacknowledged
/// addition with the same sequence number, including when a response was lost after commit.
#[derive(Clone, Default)]
pub(super) struct Outbox(Arc<Mutex<Commands>>);

impl Outbox {
    pub(super) fn enter(&self, id: &str) {
        let mut cmd = self.0.lock().unwrap();
        if cmd.room != id {
            cmd.room = id.to_owned();
            cmd.pending.clear();
            cmd.played = Played::default();
            cmd.sent_played = Played::default();
        }
    }

    pub(super) fn set_profile(&self, profile: SessionProfile) {
        self.0.lock().unwrap().profile = profile;
    }

    pub(super) fn add(&self, id: &str, song: Song) -> bool {
        let mut cmd = self.0.lock().unwrap();
        if cmd.room != id || cmd.pending.len() >= 48 {
            return false;
        }
        cmd.sequence += 1;
        let seq = cmd.sequence;
        cmd.pending.push_back(Addition { seq, song });
        true
    }

    pub(super) fn played(&self, id: &str, entry: u64, remaining: &[Entry]) {
        let mut cmd = self.0.lock().unwrap();
        if cmd.room == id {
            cmd.played.consume(entry, remaining);
        }
    }

    pub(super) fn has_changes(&self) -> bool {
        let cmd = self.0.lock().unwrap();
        !cmd.pending.is_empty() || cmd.played != cmd.sent_played || cmd.profile != cmd.sent_profile
    }

    pub(super) fn filter_played(&self, snapshot: &mut Snapshot) {
        let cmd = self.0.lock().unwrap();
        snapshot.queue.retain(|e| cmd.played.keep(e.entry));
    }

    pub(super) async fn sync(
        &self,
        request: reqwest::RequestBuilder,
        host_state: Option<&Option<Song>>,
    ) -> anyhow::Result<Snapshot> {
        let (body, seq, profile, played) = {
            let cmd = self.0.lock().unwrap();
            let mut body = serde_json::to_value(&cmd.profile)?;
            if let Some(state) = host_state {
                body["state"] = serde_json::to_value(state)?;
                body["played"] = cmd.played.through.into();
                if !cmd.played.keep_entries.is_empty() {
                    body["keep_entries"] = serde_json::to_value(&cmd.played.keep_entries)?;
                }
                if let Some(entry) = cmd.played.play_entry {
                    body["play_entry"] = entry.into();
                }
            }
            let add = cmd.pending.front();
            if let Some(add) = add {
                body["add"] = serde_json::to_value(add)?;
            }
            (
                body,
                add.map(|a| a.seq),
                cmd.profile.clone(),
                cmd.played.clone(),
            )
        };
        let res = request
            .header("X-Moosik-Session", "1")
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(&body)?)
            .send()
            .await?
            .error_for_status()?;
        let mut snapshot: Snapshot = serde_json::from_slice(&res.bytes().await?)?;
        anyhow::ensure!(
            snapshot.state.as_ref().is_none_or(Song::valid)
                && snapshot.queue.len() <= 48
                && snapshot.members.len() <= 32
                && snapshot.queue.iter().all(|e| e.song.valid())
        );
        let mut cmd = self.0.lock().unwrap();
        cmd.sent_played = played;
        cmd.sent_profile = profile;
        if let Some(seq) = seq {
            snapshot.added = snapshot.error.is_none();
            if cmd.pending.front().is_some_and(|a| a.seq == seq) {
                cmd.pending.pop_front();
            }
        }
        Ok(snapshot)
    }
}
