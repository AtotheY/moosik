//! In-process resolver built on a vendored `rusty_ytdl`.
//!
//! This is the fast path (~200ms). It runs YouTube's obfuscated player JS (signature cipher
//! and the `n` parameter) through the Boa JS engine in-process, which is why it is quick and
//! also why it is the part most likely to break when Google changes the player.
//!
//! Upstream `rusty_ytdl` has not been released since Aug 2024; we depend on the fork that
//! `youtui` publishes and keeps current, for exactly this reason.

use anyhow::{Context, Result, anyhow, bail};
use rusty_ytdl::{RequestOptions, Video, VideoOptions, VideoQuality, VideoSearchOptions};
use url::Url;

use crate::{AudioStream, Codec, StreamResolver, VideoId, parse_expiry, score};

/// Optional credentials that widen what the native path can reach.
#[derive(Debug, Clone, Default)]
pub struct NativeConfig {
    /// A PO token, if one has been obtained. Not required today for most tracks, but when
    /// YouTube tightens attestation this is the field that keeps the fast path alive.
    pub po_token: Option<String>,
    /// Cookie header from a signed-in session. Needed for age-restricted or private content.
    pub cookies: Option<String>,
}

pub struct NativeResolver {
    options: VideoOptions,
}

impl NativeResolver {
    pub fn new(config: NativeConfig) -> Self {
        Self {
            options: VideoOptions {
                // We never want video streams; asking for audio only keeps the format list
                // short and avoids paying to decipher URLs we will discard.
                quality: VideoQuality::HighestAudio,
                filter: VideoSearchOptions::Audio,
                request_options: RequestOptions {
                    po_token: config.po_token,
                    cookies: config.cookies,
                    ..Default::default()
                },
                ..Default::default()
            },
        }
    }
}

impl Default for NativeResolver {
    fn default() -> Self {
        Self::new(NativeConfig::default())
    }
}

#[async_trait::async_trait]
impl StreamResolver for NativeResolver {
    fn name(&self) -> &'static str {
        "native"
    }

    async fn resolve(&self, id: &VideoId) -> Result<AudioStream> {
        let video = Video::new_with_options(id.as_str(), self.options.clone())
            .map_err(|e| anyhow!("{e}"))
            .context("constructing video handle")?;

        let info = video
            .get_info()
            .await
            .map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("fetching player info for {id}"))?;

        let best = info
            .formats
            .iter()
            // Audio-only: a format carrying video wastes bandwidth on a music player.
            .filter(|f| f.has_audio && !f.has_video && !f.is_hls && !f.is_dash_mpd)
            .filter_map(|f| {
                let codec_str = f
                    .mime_type
                    .audio_codec
                    .as_deref()
                    .or_else(|| f.mime_type.codecs.first().map(String::as_str))?;
                let codec = Codec::from_codec_str(codec_str);
                let bitrate_kbps = (f.bitrate / 1000) as u32;
                Some((score(codec, bitrate_kbps), codec, bitrate_kbps, f))
            })
            .max_by_key(|(s, _, _, _)| *s)
            .ok_or_else(|| {
                anyhow!("no audio-only format offered for {id} (SABR-only response?)")
            })?;

        let (_, codec, bitrate_kbps, format) = best;

        if format.url.is_empty() {
            bail!(
                "format {} for {id} carried no URL (SABR-only response?)",
                format.itag
            );
        }

        let url = Url::parse(&format.url)
            .with_context(|| format!("player returned an unparseable URL for {id}"))?;

        // A resolve that "succeeds" is not the same as a playable URL.
        //
        // YouTube throttles streams whose `n` parameter has not been put through the
        // player's transform. Verified at the M1 gate: a URL without `n` serves a single
        // range starting at byte 0 and then answers 403 to every subsequent range, so mpv
        // fails a second or two into the track. rusty_ytdl only applies its n-transform when
        // the URL carries an `n` component, and the client it picks internally returns URLs
        // without one.
        //
        // Rejecting these here turns a silently-poisoned stream into an ordinary failure
        // that ChainResolver can fall through, which is the whole reason the chain exists.
        if url.query_pairs().all(|(k, _)| k != "n") {
            bail!(
                "native resolver produced a URL without an `n` parameter for {id}; \
                 it would be throttled and unplayable"
            );
        }

        Ok(AudioStream {
            itag: format.itag as u32,
            codec,
            bitrate_kbps,
            content_length: format.content_length.as_ref().and_then(|s| s.parse().ok()),
            duration: format
                .approx_duration_ms
                .as_ref()
                .and_then(|s| s.parse::<u64>().ok())
                .map(std::time::Duration::from_millis),
            expires_at: parse_expiry(&url),
            loudness_db: format.loudness_db.map(|d| d as f32),
            url,
            resolved_by: "native",
            mime: None,
            needs_ranged: false,
        })
    }
}
