//! Ephemeral, opt-in rooms. Discord only gets a read-only join URL.
use super::*;
use serde::{Deserialize, Serialize};
use std::sync::mpsc::{Receiver, Sender, channel};
use tokio::sync::watch;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Song {
    id: String,
    title: String,
    artist: String,
    position: f64,
    playing: bool,
    #[serde(default)]
    audio_alternative: bool,
}

impl Song {
    fn valid(&self) -> bool {
        moosik_resolve::VideoId::new(&self.id).is_ok()
            && self.title.len() <= 1024
            && self.artist.len() <= 1024
            && self.position.is_finite()
            && (0.0..=604800.0).contains(&self.position)
    }

    fn track(&self) -> Track {
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
    Reconnecting,
    Follow(Option<Song>, Instant),
    End(&'static str),
}

struct Session {
    updates: Receiver<Update>,
    // Closing this channel ends hosting, including while room creation is in flight.
    publish: Option<watch::Sender<Option<Song>>>,
    id: Option<String>,
    next_sample: Instant,
}

#[derive(Default)]
pub(super) struct Together {
    session: Option<Session>,
    song_request: Option<Receiver<Option<Track>>>,
    pub(super) backup: Option<crate::playback::Store>,
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
}

async fn host(
    mut samples: watch::Receiver<Option<Song>>,
    tx: Sender<Update>,
    ctx: egui::Context,
) -> anyhow::Result<()> {
    let http = client()?;
    let rooms = format!("{}/rooms", origin());
    let mut delay = Duration::from_secs(15);
    while samples.has_changed().is_ok() {
        if host_room(&http, &rooms, &mut samples, &tx, &ctx)
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

async fn host_room(
    http: &reqwest::Client,
    rooms: &str,
    samples: &mut watch::Receiver<Option<Song>>,
    tx: &Sender<Update>,
    ctx: &egui::Context,
) -> anyhow::Result<()> {
    let res = http.post(rooms).send().await?.error_for_status()?;
    let created: Created = serde_json::from_slice(&res.bytes().await?)?;
    anyhow::ensure!(created.id.len() == 32 && created.id.bytes().all(|b| b.is_ascii_hexdigit()));
    let url = format!("{rooms}/{}", created.id);
    let mut advertised = false;
    let mut retry = Duration::from_secs(2);
    loop {
        if samples.has_changed().is_err() {
            break;
        }
        let state = samples.borrow_and_update().clone();
        let sent = http
            .put(&url)
            .bearer_auth(&created.key)
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(&state)?)
            .send()
            .await
            .and_then(|r| r.error_for_status());
        if let Err(error) = sent {
            if error.status() == Some(reqwest::StatusCode::NOT_FOUND) {
                return Err(error.into());
            }
            // Keep this room through network and quota failures. A new POST spends
            // a separate shared allowance and cannot fix a rejected playback write.
            if advertised && !notify(tx, ctx, Update::Reconnecting) {
                break;
            }
            advertised = false;
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
        if samples.changed().await.is_err() {
            break;
        }
    }
    let _ = http.delete(&url).bearer_auth(&created.key).send().await;
    Ok(())
}

async fn follow(id: String, tx: Sender<Update>, ctx: egui::Context) -> anyhow::Result<()> {
    let http = client()?;
    let url = format!("{}/rooms/{id}", origin());
    let mut failures = 0;
    loop {
        match http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(res) => {
                let song: Option<Song> = serde_json::from_slice(&res.bytes().await?)?;
                anyhow::ensure!(song.as_ref().is_none_or(Song::valid));
                if !notify(&tx, &ctx, Update::Follow(song, Instant::now())) {
                    return Ok(());
                }
                failures = 0;
            }
            Err(e) if e.status() == Some(reqwest::StatusCode::NOT_FOUND) => break,
            Err(_) => {
                failures += 1;
                if failures >= 3 {
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    anyhow::bail!("session unavailable")
}

impl App {
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

    pub(crate) fn start_hosting(&mut self) {
        if !self.settings.discord_rich_presence || self.together.session.is_some() {
            return;
        }
        let (tx, updates) = channel();
        let (publish, samples) = watch::channel(None);
        let ctx = self.ctx.clone();
        self.backend.runtime().spawn(async move {
            if host(samples, tx.clone(), ctx.clone()).await.is_err() {
                notify(
                    &tx,
                    &ctx,
                    Update::End("Couldn’t connect to the Listen along service"),
                );
            }
        });
        self.together.session = Some(Session {
            updates,
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
        self.note_playback_choice();
        self.together.song_request = None;
        match link {
            crate::links::Link::Play(id) => {
                if self.following() {
                    self.leave_room();
                }
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
                self.leave_room();
                let (tx, updates) = channel();
                let ctx = self.ctx.clone();
                let room = id.clone();
                self.backend.runtime().spawn(async move {
                    if follow(room, tx.clone(), ctx.clone()).await.is_err() {
                        notify(
                            &tx,
                            &ctx,
                            Update::End("This listening session ended or is unavailable"),
                        );
                    }
                });
                self.together.session = Some(Session {
                    updates,
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
        let position = self.saved_position();
        let Some(session) = self.together.session.as_mut() else {
            return;
        };
        if let Some(publish) = &session.publish
            && Instant::now() >= session.next_sample
        {
            session.next_sample = Instant::now() + Duration::from_secs(1);
            let sample = self
                .now
                .track
                .as_ref()
                .filter(|t| {
                    t.local_path.is_none() && moosik_resolve::VideoId::new(&t.video_id).is_ok()
                })
                .map(|track| Song {
                    id: track.video_id.clone(),
                    title: track.title.chars().take(128).collect(),
                    artist: track.artist_line().chars().take(128).collect(),
                    position: position.clamp(0.0, 604800.0),
                    audio_alternative: track.wide && !self.play_is_video,
                    playing: self.playback_started
                        && !self.player_state.paused
                        && !self.player_state.idle,
                });
            publish.send_replace(sample);
        }
        self.ctx.request_repaint_after(Duration::from_secs(1));
        let mut latest = None;
        while let Ok(update) = session.updates.try_recv() {
            latest = Some(update);
        }
        match latest {
            Some(Update::Reconnecting) => {
                if session.id.take().is_some() {
                    self.presence_key = None;
                }
            }
            Some(Update::Ready(id)) => {
                session.id = Some(id);
                self.presence_key = None;
                self.toast("Listen along is open on Discord");
            }
            Some(Update::End(message)) => {
                self.leave_room();
                self.toast(message);
            }
            Some(Update::Follow(None, _)) => {
                if self.together.backup.is_some()
                    && let Some(player) = &self.player
                {
                    player.set_paused(true);
                }
            }
            Some(Update::Follow(Some(song), received)) => {
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

    /// Simulate a suspended host. The development relay expires an idle room after 45 seconds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires the default loopback relay and waits for room expiry"]
    async fn enabled_host_replaces_expired_invite() {
        assert!(origin().starts_with("http://127.0.0.1:"));
        let (tx, updates) = channel();
        let (write, samples) = watch::channel(None);
        let worker = tokio::spawn(host(samples, tx, egui::Context::default()));
        let Update::Ready(first) = updates.recv_timeout(Duration::from_secs(5)).unwrap() else {
            panic!("missing initial invite")
        };
        tokio::time::sleep(Duration::from_secs(46)).await;
        for _ in 0..3 {
            write.send_replace(None);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        assert!(matches!(
            updates.recv_timeout(Duration::from_secs(5)),
            Ok(Update::Reconnecting)
        ));
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
        };
        let (write, samples) = watch::channel(Some(song.clone()));
        let worker = tokio::spawn(host(samples, host_tx, ctx.clone()));
        let Update::Ready(room) = host_rx.recv_timeout(Duration::from_secs(8)).unwrap() else {
            panic!("missing room")
        };
        let (tx, rx) = channel();
        let guest = tokio::spawn(follow(room, tx, ctx));
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
        write.send_replace(Some(skipped.clone()));
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
