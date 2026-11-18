//! Each viewer chooses their own caption language; selection never changes room playback.
use super::App;

#[derive(Default)]
pub(crate) struct Captions {
    pub tracks: Vec<moosik_resolve::CaptionTrack>,
    pub ready: bool,
    pub selected: Option<usize>,
    pub loading: bool,
    pub request: u64,
    file: Option<tempfile::NamedTempFile>,
}

impl App {
    pub(crate) fn select_caption(&mut self, selected: Option<usize>) {
        self.captions.request = self.captions.request.wrapping_add(1);
        self.captions.selected = selected;
        if let Some(player) = &self.player {
            player.hide_captions();
        }
        self.captions.loading = false;
        if let Some(track) = selected.and_then(|index| self.captions.tracks.get(index)) {
            self.captions.loading = true;
            self.backend.load_caption(
                track.url.clone(),
                self.play_generation,
                self.captions.request,
            );
        }
    }

    pub(super) fn caption_loaded(
        &mut self,
        request: u64,
        result: Result<tempfile::NamedTempFile, String>,
    ) {
        if request != self.captions.request {
            return;
        }
        self.captions.loading = false;
        let Some(track) = self
            .captions
            .selected
            .and_then(|i| self.captions.tracks.get(i))
        else {
            return;
        };
        let result = result.and_then(|file| {
            let player = self.player.as_ref().ok_or("video player unavailable")?;
            player
                .show_captions(file.path(), &track.label, &track.language)
                .map_err(|e| e.to_string())?;
            Ok(file)
        });
        match result {
            Ok(file) => self.captions.file = Some(file),
            Err(_) => {
                self.captions.selected = None;
                self.toast_error("Couldn’t load captions. Choose a language to try again.");
            }
        }
    }
}
