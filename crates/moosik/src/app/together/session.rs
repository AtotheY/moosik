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
    pub(super) skip_target: Option<String>,
    #[serde(default)]
    pub(super) skip_requested: bool,
    #[serde(default)]
    pub(crate) can_edit_queue: bool,
    pub(crate) undo_edit: Option<u64>,
    pub current_by: Option<Member>,
    pub(super) state: Option<Song>,
    pub(super) error: Option<String>,
    #[serde(skip)]
    pub(super) added: bool,
}

impl Snapshot {
    pub(crate) fn queue_stamp(&self, room: Option<&str>) -> egui::Id {
        self.queue
            .iter()
            .fold(egui::Id::new(room), |id, entry| id.with(entry.entry))
    }

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
#[serde(untagged)]
pub(super) enum QueueEdit {
    Move {
        entries: Vec<u64>,
        before: Option<u64>,
    },
    Remove {
        entries: Vec<u64>,
    },
    Undo {
        undo: u64,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Command {
    Add {
        seq: u64,
        song: Song,
    },
    Edit {
        seq: u64,
        #[serde(flatten)]
        edit: QueueEdit,
    },
}

#[derive(Serialize)]
struct Heartbeat<'a> {
    #[serde(flatten)]
    profile: &'a SessionProfile,
    #[serde(flatten)]
    command: Option<&'a Command>,
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
    fn consume(&mut self, entry: u64, through: u64, remaining: &[Entry]) {
        self.through = self.through.max(through);
        self.keep_entries = remaining
            .iter()
            .map(|e| e.entry)
            .filter(|&id| id <= self.through)
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
    pending: VecDeque<Command>,
    consumed: Vec<u64>,
    skip: Option<String>,
    played: Played,
    sent_played: Played,
    sent_profile: SessionProfile,
}

/// UI writes into a room-scoped outbox. Each heartbeat retries its oldest unacknowledged
/// queue change with the same sequence number, including when a response was lost after commit.
#[derive(Clone, Default)]
pub(super) struct Outbox(Arc<Mutex<Commands>>);

impl Outbox {
    pub(super) fn enter(&self, id: &str) {
        let mut cmd = self.0.lock().unwrap();
        if cmd.room != id {
            cmd.room = id.to_owned();
            cmd.pending.clear();
            cmd.consumed.clear();
            cmd.skip = None;
            cmd.played = Played::default();
            cmd.sent_played = Played::default();
        }
    }

    pub(super) fn set_profile(&self, profile: SessionProfile) {
        self.0.lock().unwrap().profile = profile;
    }

    pub(super) fn add(&self, id: &str, song: Song) -> bool {
        self.enqueue(id, |seq| Command::Add { seq, song })
    }

    pub(super) fn edit(&self, room: &str, edit: QueueEdit) -> bool {
        self.enqueue(room, |seq| Command::Edit { seq, edit })
    }

    fn enqueue(&self, room: &str, command: impl FnOnce(u64) -> Command) -> bool {
        let mut cmd = self.0.lock().unwrap();
        if cmd.room != room || cmd.pending.len() >= 48 {
            return false;
        }
        cmd.sequence += 1;
        let next = command(cmd.sequence);
        cmd.pending.push_back(next);
        true
    }

    pub(super) fn request_skip(&self, room: &str, target: &str) {
        let mut cmd = self.0.lock().unwrap();
        if cmd.room == room {
            cmd.skip = Some(target.to_owned());
        }
    }

    pub(super) fn played(&self, id: &str, entry: u64, consumed: &[u64], remaining: &[Entry]) {
        let mut cmd = self.0.lock().unwrap();
        if cmd.room == id {
            cmd.played.consume(
                entry,
                consumed.iter().copied().max().unwrap_or(entry),
                remaining,
            );
            cmd.consumed.extend_from_slice(consumed);
        }
    }

    pub(super) fn has_changes(&self) -> bool {
        let cmd = self.0.lock().unwrap();
        !cmd.consumed.is_empty()
            || !cmd.pending.is_empty()
            || cmd.played != cmd.sent_played
            || cmd.profile != cmd.sent_profile
    }

    pub(super) fn filter_played(&self, snapshot: &mut Snapshot) {
        let cmd = self.0.lock().unwrap();
        if snapshot.can_edit_queue {
            snapshot.queue.retain(|e| !cmd.consumed.contains(&e.entry));
        } else {
            snapshot.queue.retain(|e| cmd.played.keep(e.entry));
        }
    }

    pub(super) async fn sync(
        &self,
        request: reqwest::RequestBuilder,
        host_state: Option<&Option<Song>>,
    ) -> anyhow::Result<Snapshot> {
        let (body, sent_add, profile, played, consumed, skip) = {
            let cmd = self.0.lock().unwrap();
            let command = cmd.pending.front();
            let mut body = serde_json::to_value(Heartbeat {
                profile: &cmd.profile,
                command,
            })?;
            if let Some(state) = host_state {
                body["played_entries"] = serde_json::to_value(&cmd.consumed)?;
                if let Some(target) = state.as_ref().and_then(|s| s.skip_target.as_deref()) {
                    body["skip_target"] = target.into();
                }
                body["state"] = serde_json::to_value(state)?;
                body["played"] = cmd.played.through.into();
                if !cmd.played.keep_entries.is_empty() {
                    body["keep_entries"] = serde_json::to_value(&cmd.played.keep_entries)?;
                }
                if let Some(entry) = cmd.played.play_entry {
                    body["play_entry"] = entry.into();
                }
            }
            if host_state.is_none()
                && let Some(target) = &cmd.skip
            {
                body["skip"] = target.as_str().into();
            }
            (
                body,
                command.map(|c| matches!(c, Command::Add { .. })),
                cmd.profile.clone(),
                cmd.played.clone(),
                cmd.consumed.clone(),
                cmd.skip.clone(),
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
        if cmd.skip == skip {
            cmd.skip = None;
        }
        cmd.consumed.retain(|id| !consumed.contains(id));
        cmd.sent_played = played;
        cmd.sent_profile = profile;
        if let Some(added) = sent_add {
            snapshot.added = added && snapshot.error.is_none();
            cmd.pending.pop_front();
        }
        Ok(snapshot)
    }
}
