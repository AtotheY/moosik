mod browse;
mod model;
mod transport;
mod verification;
mod web_auth;

pub use browse::{
    Album, AlbumSummary, Artist, ArtistCredit, ArtistSummary, CatalogItem, CatalogTrack,
    LibrarySection, Page, Playlist, PlaylistSummary, SearchKind, Section, library_start_cursor,
    resource_uri,
};
pub use model::AccountProfile;

pub use transport::{
    PlaybackReport, PlaybackReportKind, PlaybackTimeline, PreparedTrack, RangeData, TrackMetadata,
    Transport,
};
pub use web_auth::{WebSession, web_session};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Spotify {stage} request failed with HTTP {status}")]
    Http { stage: &'static str, status: u16 },
    #[error("Spotify {stage} request failed with HTTP {status} ({diagnostic})")]
    ReportRejected {
        stage: &'static str,
        status: u16,
        diagnostic: String,
    },
    #[error("Spotify {0} transport failed")]
    Network(&'static str),
    #[error("Spotify returned invalid {0}")]
    Invalid(&'static str),
    #[error("Spotify {0} timed out")]
    Timeout(&'static str),
    #[error("Spotify authorization was denied")]
    AuthorizationDenied,
    #[error("Spotify authorization expired")]
    AuthorizationExpired,
    #[error("Spotify client token could not be acquired")]
    ClientToken,
    #[error("Spotify Dealer connection closed")]
    DealerClosed,
}

pub type Result<T> = std::result::Result<T, Error>;

fn http_client() -> Result<reqwest::Client> {
    configure_tls();
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Error::Network("client setup"))
}

fn configure_tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn now_ms() -> Result<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .ok_or(Error::Invalid("system clock"))
}

fn random_device_id() -> String {
    rand::random::<[u8; 20]>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn http_and_websocket_clients_have_an_explicit_tls_provider() {
        let _http = super::http_client().unwrap();
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        let _websocket = tokio_tungstenite::Connector::Rustls(std::sync::Arc::new(config));
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
}
