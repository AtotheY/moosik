//! Ephemeral listening rooms. Invite holders can follow playback and contribute songs.
use super::*;
mod session;
use serde::{Deserialize, Serialize};
pub(crate) use session::Member;
use session::{Outbox, Snapshot};
use std::sync::mpsc::{Receiver, Sender, channel};
use tokio::sync::watch;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Song {
    id: String,
    pub title: String,
    pub artist: String,
    position: f64,
    playing: bool,
    #[serde(default)]
    audio_alternative: bool,
    /// Host-only metadata for the session heartbeat, omitted from shareable song metadata.
    #[serde(skip)]
    skip_target: Option<String>,
}

impl Song {
    fn valid(&self) -> bool {
        moosik_resolve::VideoId::new(&self.id).is_ok()
            && self.title.len() <= 1024
            && self.artist.len() <= 1024
            && self.position.is_finite()
            && (0.0..=604800.0).contains(&self.position)
    }

    pub(crate) fn track(&self) -> Track {
        crate::listening::Metadata {
            id: self.id.clone(),
            title: self.title.clone(),
            artists: if self.artist.is_empty() {
                Vec::new()
            } else {
                vec![(self.artist.clone(), None)]
            },
            video: self.audio_alternative,
            artwork: Some(format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", self.id)),
            ..Default::default()
        }
        .to_track()
    }
}

enum Update {
    Ready(String),
    Snapshot(Snapshot),
    Reconnecting,
    Follow(Option<Song>, Instant),
    End(&'static str),
}

struct Session {
    outbox: Outbox,
    snapshot: Option<Snapshot>,
    updates: Receiver<Update>,
    // Closing this channel ends hosting, including while room creation is in flight.
    publish: Option<watch::Sender<Option<Song>>>,
    id: Option<String>,
    next_sample: Instant,
}

#[derive(Default)]
pub(crate) struct Together {
    session: Option<Session>,
    song_request: Option<Receiver<Option<Track>>>,
    pub(super) backup: Option<crate::playback::Store>,
}

impl Together {
    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session.as_ref()?.id.as_deref()
    }

    pub(crate) fn snapshot(&self) -> Option<&Snapshot> {
        self.session.as_ref()?.snapshot.as_ref()
    }
}

// A generation identifies one playback even if the next entry has the same video ID.
// A manual Next also consumes the target when there is nothing left to play.
fn skip_target(generation: u64, choice: u64) -> String {
    format!("{generation:x}-{choice:x}")
}

fn origin() -> String {
    // Local integration tests can use a loopback relay. Invites cannot select the server.
    if let Ok(raw) = std::env::var("MOOSIK_LOCAL_RELAY")
        && let Ok(url) = url::Url::parse(&raw)
        && url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
    {
        return url.origin().ascii_serialization();
    }
    "https://rooms.moosik.dev".into()
}

fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?)
}

fn notify(tx: &Sender<Update>, ctx: &egui::Context, update: Update) -> bool {
    let sent = tx.send(update).is_ok();
    if sent {
        ctx.request_repaint();
    }
    sent
}

#[derive(Deserialize)]
struct Created {
    id: String,
    key: String,
    #[serde(default)]
    session: Option<String>,
}

async fn host(
    mut samples: watch::Receiver<Option<Song>>,
    outbox: Outbox,
    tx: Sender<Update>,
    ctx: egui::Context,
) -> anyhow::Result<()> {
    let http = client()?;
    let rooms = format!("{}/rooms", origin());
    let mut delay = Duration::from_secs(15);
    while samples.has_changed().is_ok() {
        if host_room(&http, &rooms, &mut samples, &outbox, &tx, &ctx)
            .await
            .is_ok()
        {
            break;
        }
        if !notify(&tx, &ctx, Update::Reconnecting) {
            break;
        }
        // Song changes during backoff are coalesced. Turning hosting off cancels the wait.
        tokio::select! {
            _ = samples.wait_for(|_| false) => break,
            _ = tokio::time::sleep(delay) => {}
        }
        delay = (delay * 2).min(Duration::from_secs(120));
    }
    Ok(())
}

/// Between transport changes the relay extrapolates the position. A solo host only
/// needs to renew its 45-second lease and discover new listeners.
fn solo_heartbeat_due(last: &Option<Song>, next: &Option<Song>, elapsed: Duration) -> bool {
    if elapsed >= Duration::from_secs(15) {
        return true;
    }
    match (last, next) {
        (Some(a), Some(b)) => {
            let projected = a.position
                + if a.playing {
                    elapsed.as_secs_f64()
                } else {
                    0.0
                };
            a.skip_target != b.skip_target
                || a.id != b.id
                || a.title != b.title
                || a.artist != b.artist
                || a.playing != b.playing
                || a.audio_alternative != b.audio_alternative
                || (b.position - projected).abs() > 1.5
        }
        (None, None) => false,
        _ => true,
    }
}

async fn host_room(
    http: &reqwest::Client,
    rooms: &str,
    samples: &mut watch::Receiver<Option<Song>>,
    outbox: &Outbox,
    tx: &Sender<Update>,
    ctx: &egui::Context,
) -> anyhow::Result<()> {
    let res = http.post(rooms).send().await?.error_for_status()?;
    let created: Created = serde_json::from_slice(&res.bytes().await?)?;
    anyhow::ensure!(created.id.len() == 32 && created.id.bytes().all(|b| b.is_ascii_hexdigit()));
    let url = format!("{rooms}/{}", created.id);
    outbox.enter(&created.id);
    let mut last_solo_sample: Option<(Option<Song>, Instant)> = None;
    let mut advertised = false;
    let mut retry = Duration::from_secs(2);
    loop {
        if samples.has_changed().is_err() {
            break;
        }
        let state = samples.borrow_and_update().clone();
        if let Some((prior, sent_at)) = &last_solo_sample
            && !solo_heartbeat_due(prior, &state, sent_at.elapsed())
            && (created.session.as_deref() != Some("1") || !outbox.has_changes())
        {
            if samples.changed().await.is_err() {
                break;
            }
            continue;
        }
        let sampled_at = Instant::now();
        let request = http.put(&url).bearer_auth(&created.key);
        let sent = if created.session.as_deref() == Some("1") {
            outbox.sync(request, Some(&state)).await.map(Some)
        } else {
            request
                .header("Content-Type", "application/json")
                .body(serde_json::to_vec(&state)?)
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map(|_| None)
                .map_err(Into::into)
        };
        if let Err(error) = &sent {
            if error
                .downcast_ref::<reqwest::Error>()
                .and_then(|e| e.status())
                == Some(reqwest::StatusCode::NOT_FOUND)
            {
                anyhow::bail!("room expired");
            }
            // Keep this room through network and quota failures. A new POST spends
            // a separate shared allowance and cannot fix a rejected playback write.
            if advertised && !notify(tx, ctx, Update::Reconnecting) {
                break;
            }
            advertised = false;
            last_solo_sample = None;
            tokio::select! {
                _ = samples.wait_for(|_| false) => break,
                _ = tokio::time::sleep(retry) => {}
            }
            retry = (retry * 2).min(Duration::from_secs(120));
            continue;
        }
        retry = Duration::from_secs(2);
        if !advertised {
            advertised = notify(tx, ctx, Update::Ready(created.id.clone()));
            if !advertised {
                break;
            }
        }
        let solo = match &sent {
            Ok(Some(snapshot)) => snapshot.members.len() == 1,
            Ok(None) => true,
            Err(_) => false,
        };
        last_solo_sample = solo.then_some((state, sampled_at));
        if let Ok(Some(snapshot)) = sent
            && !notify(tx, ctx, Update::Snapshot(snapshot))
        {
            break;
        }
        if samples.changed().await.is_err() {
            break;
        }
    }
    let _ = http.delete(&url).bearer_auth(&created.key).send().await;
    Ok(())
}

async fn follow(
    id: String,
    outbox: Outbox,
    tx: Sender<Update>,
    ctx: egui::Context,
) -> anyhow::Result<()> {
    let http = client()?;
    let url = format!("{}/rooms/{id}", origin());
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    let token = u128::from_be_bytes(bytes);
    let token = format!("{token:032x}");
    let guest = || http.put(&url).header("X-Moosik-Listener", &token);
    outbox.enter(&id);
    let mut session = false;
    let mut failures = 0;
    loop {
        let result: anyhow::Result<Option<Song>> = async {
            if session {
                let snapshot = outbox.sync(guest(), None).await?;
                let song = snapshot.state.clone();
                notify(&tx, &ctx, Update::Snapshot(snapshot));
                Ok(song)
            } else {
                let res = http.get(&url).send().await?.error_for_status()?;
                session = res
                    .headers()
                    .get("X-Moosik-Session")
                    .is_some_and(|v| v == "1");
                let song: Option<Song> = serde_json::from_slice(&res.bytes().await?)?;
                anyhow::ensure!(song.as_ref().is_none_or(Song::valid));
                Ok(song)
            }
        }
        .await;
        match result {
            Ok(song) => {
                if !notify(&tx, &ctx, Update::Follow(song, Instant::now())) {
                    break;
                }
                failures = 0;
            }
            Err(e) => {
                failures += 1;
                if failures >= 3
                    || e.downcast_ref::<reqwest::Error>().and_then(|e| e.status())
                        == Some(reqwest::StatusCode::NOT_FOUND)
                {
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if session {
        let _ = guest()
            .header("X-Moosik-Session", "1")
            .body(r#"{"name":"","leave":true}"#)
            .send()
            .await;
    }
    anyhow::bail!("session unavailable")
}

impl App {
    fn room_profile(&self) -> crate::discord::SessionProfile {
        let mut profile = self
            .discord
            .as_ref()
            .map(|d| d.session_profile())
            .unwrap_or_default();
        if !self.settings.session_name.trim().is_empty() {
            profile.name = self.settings.session_name.chars().take(32).collect();
        }
        profile
    }

    fn room_outbox(&self) -> Outbox {
        let outbox = Outbox::default();
        outbox.set_profile(self.room_profile());
        outbox
    }

    pub(crate) fn room_snapshot(&self) -> Option<&Snapshot> {
        self.together.snapshot()
    }

    pub(crate) fn add_session_song(&mut self, track: &Track) {
        let Some(session) = &self.together.session else {
            return;
        };
        let Some(id) = &session.id else { return };
        let song = Song {
            id: track.video_id.clone(),
            title: track.title.chars().take(128).collect(),
            artist: track.artist_line().chars().take(128).collect(),
            position: 0.0,
            playing: false,
            audio_alternative: track.wide,
            skip_target: None,
        };
        if !track.can_share_in_session() || !song.valid() {
            self.toast("Only YouTube Music tracks can be shared");
        } else if !session.outbox.add(id, song) {
            self.toast("Couldn’t add this song to the session");
        }
    }

    pub(super) fn request_session_skip(&mut self) {
        let Some(session) = &self.together.session else {
            return;
        };
        let target = session
            .snapshot
            .as_ref()
            .and_then(|s| s.skip_target.as_deref());
        if let Some((id, target)) = session.id.as_deref().zip(target) {
            session.outbox.request_skip(id, target);
        } else {
            self.toast("Skipping is unavailable in this session");
        }
    }

    pub(super) fn play_session_next(&mut self) -> bool {
        let Some(entry) = self.room_snapshot().and_then(|room| room.queue.first()) else {
            return false;
        };
        self.consume_session_entry(entry.entry)
    }

    pub(crate) fn play_session_entry(&mut self, entry_id: u64) {
        self.note_playback_choice();
        self.consume_session_entry(entry_id);
    }

    fn consume_session_entry(&mut self, entry_id: u64) -> bool {
        let Some(session) = self.together.session.as_mut() else {
            return false;
        };
        if session.publish.is_none() {
            return false;
        }
        let Some((id, snapshot)) = session.id.as_ref().zip(session.snapshot.as_mut()) else {
            return false;
        };
        let Some(index) = snapshot
            .queue
            .iter()
            .position(|entry| entry.entry == entry_id)
        else {
            return false;
        };
        let track = snapshot.queue[index].song.track();
        // Older rooms only understand a FIFO acknowledgement. A host who keeps skipped songs
        // can still play the choice immediately there without discarding anyone's contributions.
        if index == 0 || !self.settings.queue_click_keeps_order || snapshot.can_pick_entry {
            if self.settings.queue_click_keeps_order {
                snapshot.queue.remove(index);
            } else {
                snapshot.queue.drain(..=index);
            }
            session.outbox.played(id, entry_id, &snapshot.queue);
        }
        let index = self.q.current().map_or(0, |i| i + 1);
        if let Some(radio) = &mut self.radio {
            radio.chosen.insert(track.video_id.clone());
        }
        self.q.insert(
            index,
            track,
            Slot::Next(0),
            self.radio.as_ref().map(|r| &r.chosen),
        );
        self.play_index(index);
        true
    }

    pub(crate) fn hosting(&self) -> bool {
        self.together
            .session
            .as_ref()
            .is_some_and(|s| s.publish.is_some())
    }

    pub(crate) fn following(&self) -> bool {
        self.together
            .session
            .as_ref()
            .is_some_and(|s| s.publish.is_none())
    }

    pub(super) fn room_id(&self) -> Option<&str> {
        self.together
            .session
            .as_ref()
            .filter(|s| s.publish.is_some())?
            .id
            .as_deref()
    }

    fn start_hosting(&mut self) {
        let (tx, updates) = channel();
        let (publish, samples) = watch::channel(None);
        let outbox = self.room_outbox();
        let worker = outbox.clone();
        let ctx = self.ctx.clone();
        self.backend.runtime().spawn(async move {
            if host(samples, worker, tx.clone(), ctx.clone())
                .await
                .is_err()
            {
                notify(
                    &tx,
                    &ctx,
                    Update::End("Couldn’t connect to the Listen along service"),
                );
            }
        });
        self.together.session = Some(Session {
            updates,
            outbox,
            snapshot: None,
            publish: Some(publish),
            id: None,
            next_sample: Instant::now(),
        });
    }

    pub(crate) fn leave_room(&mut self) {
        self.together.session = None;
        self.presence_key = None;
        if let Some(mut store) = self.together.backup.take() {
            // Invalidate in-flight guest stream resolution before restoring the previous queue.
            self.play_generation = self.backend.cancel_play();
            self.listening.finish("left_room");
            self.stall = Default::default();
            self.q = PlayQueue::new(unix_now() as u64);
            store.volume = self.last_volume;
            store.muted = self.muted;
            self.playback_started = false;
            if let Some(player) = &self.player {
                player.stop();
            }
            self.now = Default::default();
            self.radio = None;
            self.apply_playback_store(store);
            self.save_playback();
        }
    }

    pub(super) fn open_shared_link(&mut self, link: crate::links::Link) {
        match link {
            crate::links::Link::Play(id) => {
                self.choose_personal_playback();
                self.together.song_request = Some(self.backend.shared_song(id));
            }
            crate::links::Link::Join(id) => {
                if self
                    .together
                    .session
                    .as_ref()
                    .is_some_and(|s| s.id.as_ref() == Some(&id))
                {
                    return;
                }
                self.note_playback_choice();
                self.together.song_request = None;
                self.pending_card_play = None;
                if matches!(
                    self.pending_page_action,
                    Some((
                        _,
                        crate::ui::PageThen::StartMix | crate::ui::PageThen::Shuffle
                    ))
                ) {
                    self.pending_page_action = None;
                }
                self.autoplay = false;
                self.leave_room();
                let (tx, updates) = channel();
                let ctx = self.ctx.clone();
                let room = id.clone();
                let outbox = self.room_outbox();
                let worker = outbox.clone();
                self.backend.runtime().spawn(async move {
                    if follow(room, worker, tx.clone(), ctx.clone()).await.is_err() {
                        notify(
                            &tx,
                            &ctx,
                            Update::End("This listening session ended or is unavailable"),
                        );
                    }
                });
                self.together.session = Some(Session {
                    updates,
                    outbox,
                    snapshot: None,
                    publish: None,
                    id: Some(id),
                    next_sample: Instant::now(),
                });
            }
        }
    }

    pub(super) fn poll_together(&mut self) {
        if let Some(result) = self
            .together
            .song_request
            .as_ref()
            .and_then(|r| r.try_recv().ok())
        {
            self.together.song_request = None;
            match result {
                Some(track) => self.start_queue(vec![track], 0, None),
                None => self.toast("This shared song is unavailable"),
            }
        }
        let shareable = Track::can_share_in_session;
        let can_host = self.settings.discord_host_listen_along
            && self
                .discord
                .as_ref()
                .is_some_and(|discord| discord.connected());
        if self.hosting() && !can_host {
            self.leave_room();
            return;
        }
        if can_host
            && self.together.session.is_none()
            && self.now.track.as_ref().is_some_and(shareable)
        {
            self.start_hosting();
        }
        let position = self.saved_position();
        let profile = self
            .together
            .session
            .as_ref()
            .filter(|s| Instant::now() >= s.next_sample)
            .map(|_| self.room_profile());
        let Some(session) = self.together.session.as_mut() else {
            return;
        };
        if let Some(profile) = profile {
            session.next_sample = Instant::now() + Duration::from_secs(1);
            session.outbox.set_profile(profile);
            if let Some(publish) = &session.publish {
                let sample = self
                    .now
                    .track
                    .as_ref()
                    .filter(|t| shareable(t))
                    .map(|track| Song {
                        id: track.video_id.clone(),
                        title: track.title.chars().take(128).collect(),
                        artist: track.artist_line().chars().take(128).collect(),
                        position: position.clamp(0.0, 604800.0),
                        audio_alternative: track.wide && !self.play_is_video,
                        skip_target: Some(skip_target(self.play_generation, self.playback_choice)),
                        playing: self.playback_started
                            && !self.player_state.paused
                            && !self.player_state.idle,
                    });
                publish.send_replace(sample);
            }
        }
        self.ctx.request_repaint_after(Duration::from_secs(1));
        let updates = session.updates.try_iter().collect::<Vec<_>>();
        let mut latest = None;
        for update in updates {
            let Some(session) = &mut self.together.session else {
                break;
            };
            match update {
                Update::Snapshot(mut snapshot) => {
                    session.outbox.filter_played(&mut snapshot);
                    let message = snapshot
                        .error
                        .take()
                        .or_else(|| snapshot.added.then(|| "Added to session queue".to_owned()));
                    let skip = session.publish.is_some()
                        && snapshot.skip_requested
                        && snapshot.skip_target
                            == Some(skip_target(self.play_generation, self.playback_choice))
                        && snapshot
                            .state
                            .as_ref()
                            .zip(self.q.current_track())
                            .is_some_and(|(song, track)| song.id == track.video_id);
                    session.snapshot = Some(snapshot);
                    if skip {
                        self.skip(1);
                    }
                    if let Some(message) = message {
                        self.toast(&message);
                    }
                }
                Update::Reconnecting => {
                    session.id = None;
                    session.snapshot = None;
                    self.presence_key = None;
                }
                Update::Ready(id) => {
                    session.id = Some(id);
                    self.presence_key = None;
                    self.toast("Listen along is open on Discord");
                }
                Update::End(message) => {
                    self.leave_room();
                    self.toast(message);
                    return;
                }
                Update::Follow(song, received) => latest = Some((song, received)),
            }
        }
        match latest {
            Some((None, _)) => {
                if self.together.backup.is_some()
                    && let Some(player) = &self.player
                {
                    player.set_paused(true);
                }
            }
            Some((Some(song), received)) => {
                if self.together.backup.is_none() {
                    self.together.backup = self.playback_store();
                    self.radio = None;
                }
                let position = song.position
                    + if song.playing {
                        received.elapsed().as_secs_f64()
                    } else {
                        0.0
                    };
                if self
                    .now
                    .track
                    .as_ref()
                    .is_none_or(|t| t.video_id != song.id)
                    || (!self.playback_started
                        && !self.now.resolving
                        && song.playing
                        && self.player_error.is_none())
                {
                    self.radio = None;
                    self.q
                        .restore_session(vec![song.track()], Some(0), None, false, None);
                    self.q.set_resume(Some((0, position)));
                    self.video_open = false;
                    if song.playing {
                        self.play_index_restore(0);
                    } else {
                        self.play_generation = self.backend.cancel_play();
                        if let Some(player) = &self.player {
                            player.stop();
                        }
                        self.playback_started = false;
                        self.stall = Default::default();
                        self.now = NowPlaying {
                            track: Some(song.track()),
                            paused: true,
                            ..Default::default()
                        };
                    }
                } else if self.playback_started && !self.player_state.idle {
                    if (position - self.player_state.position).abs() > 1.5 {
                        self.seek_to(position);
                    }
                    if song.playing == self.player_state.paused
                        && let Some(player) = &self.player
                    {
                        player.set_paused(!song.playing);
                    }
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_room_cannot_open_local_paths_or_seek_to_invalid_positions() {
        let mut song = Song {
            id: "dQw4w9WgXcQ".into(),
            title: "Promiscuous".into(),
            artist: "Nelly Furtado".into(),
            position: 42.0,
            playing: true,
            audio_alternative: false,
            skip_target: None,
        };
        assert!(song.valid());
        assert!(song.track().local_path.is_none());
        song.position = f64::INFINITY;
        assert!(!song.valid());
        song.position = -1.0;
        assert!(!song.valid());
        song.position = 0.0;
        song.id = "/etc/passwd".into();
        assert!(!song.valid());
    }

    #[test]
    fn a_solo_host_renews_its_lease_and_publishes_transport_changes_promptly() {
        let song = Song {
            id: "dQw4w9WgXcQ".into(),
            title: "Track".into(),
            artist: String::new(),
            position: 10.0,
            playing: true,
            audio_alternative: false,
            skip_target: None,
        };
        let last = Some(song.clone());
        let elapsed = Duration::from_secs(5);
        let next = |position, playing| {
            Some(Song {
                position,
                playing,
                ..song.clone()
            })
        };
        assert!(!solo_heartbeat_due(&last, &next(15.0, true), elapsed));
        assert!(solo_heartbeat_due(&last, &next(35.0, true), elapsed));
        assert!(solo_heartbeat_due(&last, &next(15.0, false), elapsed));
        assert!(solo_heartbeat_due(&last, &None, elapsed));
        assert!(!solo_heartbeat_due(&None, &None, elapsed));
        assert!(solo_heartbeat_due(&None, &None, Duration::from_secs(15)));
    }

    /// Count successful room writes while the UI continues sampling a paused track every second.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a loopback session relay and waits for a solo heartbeat"]
    async fn a_queued_song_does_not_keep_a_solo_host_polling_each_second() {
        assert!(origin().starts_with("http://127.0.0.1:"));
        let song = Song {
            id: "dQw4w9WgXcQ".into(),
            title: "Solo".into(),
            artist: String::new(),
            position: 10.0,
            playing: false,
            audio_alternative: false,
            skip_target: None,
        };
        let (tx, updates) = channel();
        let (write, samples) = watch::channel(Some(song.clone()));
        let outbox = Outbox::default();
        let worker = tokio::spawn(host(samples, outbox.clone(), tx, egui::Context::default()));
        let timeout = Duration::from_secs(5);
        let Update::Ready(room) = updates.recv_timeout(timeout).unwrap() else {
            panic!("missing invite");
        };
        assert!(matches!(
            updates.recv_timeout(timeout).unwrap(),
            Update::Snapshot(_)
        ));
        assert!(outbox.add(&room, song.clone()));
        write.send_replace(Some(song.clone()));
        let Update::Snapshot(added) = updates.recv_timeout(timeout).unwrap() else {
            panic!("missing queue");
        };
        assert_eq!(added.members.len(), 1);
        assert_eq!(added.queue.len(), 1);

        for _ in 0..17 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            write.send_replace(Some(song.clone()));
        }
        drop(write);
        worker.await.unwrap().unwrap();
        assert_eq!(
            updates
                .try_iter()
                .filter(|u| matches!(u, Update::Snapshot(_)))
                .count(),
            1
        );
    }

    /// Simulate a suspended host. The development relay expires an idle room after 45 seconds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires the default loopback relay and waits for room expiry"]
    async fn enabled_host_replaces_expired_invite() {
        assert!(origin().starts_with("http://127.0.0.1:"));
        let (tx, updates) = channel();
        let (write, samples) = watch::channel(None);
        let worker = tokio::spawn(host(
            samples,
            Outbox::default(),
            tx,
            egui::Context::default(),
        ));
        let Update::Ready(first) = updates.recv_timeout(Duration::from_secs(5)).unwrap() else {
            panic!("missing initial invite")
        };
        tokio::time::sleep(Duration::from_secs(46)).await;
        for _ in 0..3 {
            write.send_replace(None);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        while !matches!(
            updates.recv_timeout(Duration::from_secs(5)).unwrap(),
            Update::Reconnecting
        ) {}
        let Update::Ready(second) = updates.recv_timeout(Duration::from_secs(25)).unwrap() else {
            panic!("missing replacement invite")
        };
        assert_ne!(first, second);
        drop(write);
        worker.await.unwrap().unwrap();
    }

    /// Run against the real Node relay with synthetic songs, independent of Discord or mpv.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a local relay or explicit MOOSIK_TEST_PUBLIC_ROOMS=1"]
    async fn native_host_and_guest_follow_a_room() {
        assert!(
            origin().starts_with("http://127.0.0.1:")
                || std::env::var("MOOSIK_TEST_PUBLIC_ROOMS").as_deref() == Ok("1")
        );
        let ctx = egui::Context::default();
        let (host_tx, host_rx) = channel();
        let song = Song {
            id: "dQw4w9WgXcQ".into(),
            title: "Synthetic song".into(),
            artist: "Test".into(),
            position: 13.0,
            playing: false,
            audio_alternative: false,
            skip_target: None,
        };
        let hosted = Song {
            skip_target: Some(skip_target(1, 0)),
            ..song.clone()
        };
        let (write, samples) = watch::channel(Some(hosted.clone()));
        let host_commands = Outbox::default();
        let worker = tokio::spawn(host(samples, host_commands.clone(), host_tx, ctx.clone()));
        let Update::Ready(room) = host_rx.recv_timeout(Duration::from_secs(8)).unwrap() else {
            panic!("missing room")
        };
        let (tx, rx) = channel();
        let guest_commands = Outbox::default();
        guest_commands.set_profile(crate::discord::SessionProfile {
            name: "Guest".into(),
            ..Default::default()
        });
        let guest = tokio::spawn(follow(room.clone(), guest_commands.clone(), tx, ctx));
        let Update::Follow(first, _) = rx.recv_timeout(Duration::from_secs(8)).unwrap() else {
            panic!("missing snapshot")
        };
        assert_eq!(first, Some(song.clone()));
        let skipped = Song {
            id: "abcdefghijk".into(),
            position: 30.0,
            playing: true,
            ..song
        };
        if origin().starts_with("http://127.0.0.1:") {
            assert!(guest_commands.add(&room, skipped.clone()));
            let until = Instant::now() + Duration::from_secs(8);
            loop {
                let update = rx
                    .recv_timeout(until.saturating_duration_since(Instant::now()))
                    .unwrap();
                if let Update::Snapshot(s) = update
                    && let Some(entry) = s.queue.first()
                {
                    assert_eq!(entry.by.name, "Guest");
                    assert_eq!(entry.song.id, skipped.id);
                    assert_eq!(s.members.len(), 2);
                    guest_commands.request_skip(
                        &room,
                        s.skip_target
                            .as_deref()
                            .expect("host advertised skip support"),
                    );
                    break;
                }
            }
            // Wake the solo host, then drive its normal heartbeats until the guest's
            // Next reaches it. Use the queue entry that arrived on that same heartbeat.
            host_commands.set_profile(crate::discord::SessionProfile {
                name: "Host".into(),
                ..Default::default()
            });
            let until = Instant::now() + Duration::from_secs(8);
            loop {
                assert!(Instant::now() < until, "missing guest skip");
                write.send_replace(Some(hosted.clone()));
                if let Ok(Update::Snapshot(s)) = host_rx.recv_timeout(Duration::from_millis(300))
                    && s.skip_requested
                    && s.skip_target == hosted.skip_target
                {
                    host_commands.played(&room, s.queue[0].entry, &s.queue[1..]);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        write.send_replace(Some(Song {
            skip_target: Some(skip_target(2, 1)),
            ..skipped.clone()
        }));
        let until = Instant::now() + Duration::from_secs(8);
        loop {
            if let Update::Follow(Some(next), _) = rx
                .recv_timeout(until.saturating_duration_since(Instant::now()))
                .unwrap()
                && next.id == skipped.id
            {
                assert!(next.playing && next.position >= 30.0 && next.position < 38.0);
                break;
            }
        }
        drop(write);
        worker.await.unwrap().unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), guest)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }
}
