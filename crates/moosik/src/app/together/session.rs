use super::*;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Member {
    pub id: u64,
    pub name: String,
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
    pub current_by: Option<Member>,
    pub(super) state: Option<Song>,
    pub(super) error: Option<String>,
    #[serde(skip)]
    pub(super) added: bool,
}

#[derive(Clone, Serialize)]
struct Addition {
    seq: u64,
    song: Song,
}

#[derive(Default)]
struct Commands {
    room: String,
    name: String,
    sequence: u64,
    pending: VecDeque<Addition>,
    played: u64,
    sent_played: u64,
    sent_name: String,
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
            cmd.played = 0;
            cmd.sent_played = 0;
        }
    }

    pub(super) fn set_name(&self, name: String) {
        self.0.lock().unwrap().name = name;
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

    pub(super) fn played(&self, id: &str, entry: u64) {
        let mut cmd = self.0.lock().unwrap();
        if cmd.room == id {
            cmd.played = entry;
        }
    }

    pub(super) fn has_changes(&self) -> bool {
        let cmd = self.0.lock().unwrap();
        !cmd.pending.is_empty() || cmd.played != cmd.sent_played || cmd.name != cmd.sent_name
    }

    pub(super) fn last_played(&self) -> u64 {
        self.0.lock().unwrap().played
    }

    pub(super) async fn sync(
        &self,
        request: reqwest::RequestBuilder,
        host_state: Option<&Option<Song>>,
    ) -> anyhow::Result<Snapshot> {
        let (body, seq) = {
            let cmd = self.0.lock().unwrap();
            let mut body = serde_json::json!({ "name": cmd.name });
            if let Some(state) = host_state {
                body["state"] = serde_json::to_value(state)?;
                body["played"] = cmd.played.into();
            }
            let add = cmd.pending.front();
            if let Some(add) = add {
                body["add"] = serde_json::to_value(add)?;
            }
            (body, add.map(|a| a.seq))
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
        cmd.sent_played = body["played"].as_u64().unwrap_or(0);
        cmd.sent_name = body["name"].as_str().unwrap_or_default().to_owned();
        if let Some(seq) = seq {
            snapshot.added = snapshot.error.is_none();
            if cmd.pending.front().is_some_and(|a| a.seq == seq) {
                cmd.pending.pop_front();
            }
        }
        Ok(snapshot)
    }
}
