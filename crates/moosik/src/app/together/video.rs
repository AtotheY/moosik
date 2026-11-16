//! Pasted uploads travel over the existing room protocol. A source credit containing the
//! exact canonical watch URL identifies a video even on v1 relays (which discard new fields).
//! Older clients can still play its original audio. No URL from the relay is ever opened.
use super::{Song, client};
use moosik_resolve::VideoId;
use url::Url;

pub(super) fn from_link(raw: &str) -> Option<VideoId> {
    let raw = raw.trim();
    let url = Url::parse(raw)
        .or_else(|_| Url::parse(&format!("https://{raw}")))
        .ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    let id = match url.host_str()? {
        "youtu.be" => path.strip_prefix('/')?.to_owned(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            if path == "/watch" {
                url.query_pairs()
                    .find(|(key, _)| key == "v")?
                    .1
                    .into_owned()
            } else {
                path.strip_prefix("/shorts/")
                    .or_else(|| path.strip_prefix("/embed/"))?
                    .to_owned()
            }
        }
        _ => return None,
    };
    VideoId::new(id).ok()
}

impl Song {
    pub(crate) fn is_watch_video(&self) -> bool {
        self.artist.strip_prefix("https://www.youtube.com/watch?v=") == Some(self.id.as_str())
    }

    pub(super) fn upload(id: &VideoId, title: Option<&str>) -> Self {
        Self {
            id: id.to_string(),
            artist: id.video_url(),
            title: title
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.chars().take(128).collect())
                .unwrap_or_else(|| format!("YouTube video · {id}")),
            position: 0.0,
            playing: false,
            audio_alternative: false,
            skip_target: None,
        }
    }
}

pub(super) async fn lookup(id: VideoId) -> Song {
    let title = async {
        #[derive(serde::Deserialize)]
        struct Info {
            title: String,
        }
        let response = client()?
            .get("https://www.youtube.com/oembed")
            .query(&[("url", id.video_url().as_str()), ("format", "json")])
            .send()
            .await?
            .error_for_status()?;
        anyhow::Ok(serde_json::from_slice::<Info>(&response.bytes().await?)?.title)
    }
    .await
    .ok();
    Song::upload(&id, title.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_youtube_video_links_become_room_entries() {
        for link in [
            " https://youtu.be/dQw4w9WgXcQ?t=32 ",
            "www.youtube.com/watch?list=123&v=dQw4w9WgXcQ",
            "https://m.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
        ] {
            assert_eq!(from_link(link).unwrap().as_str(), "dQw4w9WgXcQ");
        }
        for link in [
            "",
            "https://youtube.com.example/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/no",
            "https://youtube.com/playlist?list=dQw4w9WgXcQ",
            "https://youtube.com@evil.example/watch?v=dQw4w9WgXcQ",
            "file:///tmp/dQw4w9WgXcQ",
        ] {
            assert!(from_link(link).is_none(), "{link}");
        }
    }
}
