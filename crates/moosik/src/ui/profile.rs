use std::sync::Arc;

use egui::RichText;

use crate::app::{App, Route};
use crate::auth::AuthState;
use crate::model::{AccountProfile, Artist, Entry, PlaylistSource, Provider, Shelf};
use crate::{theme, widgets};

const PREVIEW_TRACKS: usize = 6;
const PREVIEW_PLAYLISTS: usize = 12;

#[derive(Default)]
pub(crate) struct State {
    pub spotify: Option<AccountProfile>,
    pub spotify_loading: bool,
    pub spotify_error: Option<String>,
    pub youtube: Option<Artist>,
    pub youtube_id: Option<String>,
    pub youtube_loading: bool,
    pub youtube_error: Option<String>,
    filter: Option<Provider>,
    entries: Vec<Entry>,
    rows: super::source_rows::State,
    revision: u64,
}

impl State {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

struct Identity {
    provider: Provider,
    name: String,
    thumbnail: Option<Arc<str>>,
    audience: Option<String>,
    youtube_id: Option<String>,
    url: Option<String>,
}

pub(crate) fn page(app: &mut App, ui: &mut egui::Ui) {
    app.load_profile();
    let available = app.connected_providers();
    let youtube = available.contains(&Provider::YouTubeMusic);
    let spotify = available.contains(&Provider::Spotify);
    let local = app.has_local_music();
    if available.len() <= 1
        || app
            .profile
            .filter
            .is_some_and(|provider| !available.contains(&provider))
    {
        app.profile.filter = None;
    }
    super::page_scroll(app, "profile").show(ui, |ui| {
        super::page_title(ui, &app.palette, "Your profile");
        if available.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                if widgets::chip(ui, &app.palette, "All", app.profile.filter.is_none()).clicked() {
                    app.profile.filter = None;
                }
                for &provider in &available {
                    if widgets::chip(
                        ui,
                        &app.palette,
                        provider.name(),
                        app.profile.filter == Some(provider),
                    )
                    .clicked()
                    {
                        app.profile.filter = Some(provider);
                    }
                }
            });
            ui.add_space(12.0);
        }
        let filter = app.profile.filter;
        let visible = |provider| filter.is_none_or(|selected| selected == provider);
        let identities = identities(app, &available);
        let preferred = identities
            .iter()
            .position(|identity| {
                visible(identity.provider) && identity.provider == app.settings.preferred_provider
            })
            .or_else(|| {
                identities
                    .iter()
                    .position(|identity| visible(identity.provider))
            });
        if let Some(index) = preferred {
            identity_header(app, ui, &identities[index]);
            for (other, identity) in identities.iter().enumerate() {
                if other != index && visible(identity.provider) {
                    identity_row(app, ui, identity);
                }
            }
        }
        for provider in [Provider::YouTubeMusic, Provider::Spotify] {
            if available.contains(&provider) && visible(provider) {
                provider_status(app, ui, provider);
            }
        }
        if youtube && visible(Provider::YouTubeMusic) {
            youtube_sections(app, ui);
        }
        for &provider in &available {
            if visible(provider) {
                playlist_shelf(app, ui, provider);
            }
        }
        saved_songs(app, ui, &available);
        if local && (filter == Some(Provider::Local) || available == [Provider::Local]) {
            if app.local_scanning {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Scanning local music…")
                        .font(theme::regular(12.0))
                        .color(app.palette.secondary),
                );
            } else if app.local_tracks.is_empty() && app.local_playlists.is_empty() {
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Your local library is empty.")
                        .font(theme::regular(13.0))
                        .color(app.palette.secondary),
                );
                if widgets::outline_button(ui, &app.palette, "Add folders").clicked() {
                    app.settings_tab = super::settings::SettingsTab::Providers;
                    app.settings_provider = Provider::Local;
                    app.navigate(Route::Settings);
                }
            }
            if let Some(error) = &app.local_scan_error {
                ui.label(
                    RichText::new(error)
                        .font(theme::regular(12.0))
                        .color(app.palette.danger),
                );
            }
        }
        if app.library_loading && (youtube || spotify) && filter != Some(Provider::Local) {
            super::section(ui, &app.palette, "Saved music");
            widgets::skeleton_track_rows(ui, &app.palette, 3, false);
        }
        if let Some(error) = app
            .library_error
            .as_ref()
            .filter(|_| (youtube || spotify) && filter != Some(Provider::Local))
        {
            ui.add_space(12.0);
            ui.label(
                RichText::new(error)
                    .font(theme::regular(12.0))
                    .color(app.palette.danger),
            );
            if widgets::outline_button(ui, &app.palette, "Try again").clicked() {
                app.retry_profile();
            }
        }
        if available.is_empty() {
            ui.label(
                RichText::new("Set up your music in Settings.")
                    .font(theme::regular(13.0))
                    .color(app.palette.secondary),
            );
            ui.add_space(12.0);
            if widgets::pill_button(ui, &app.palette, "Manage providers").clicked() {
                app.settings_tab = super::settings::SettingsTab::Providers;
                app.settings_provider = Provider::YouTubeMusic;
                app.navigate(Route::Settings);
            }
        } else {
            ui.add_space(20.0);
            if widgets::outline_button(ui, &app.palette, "Open library").clicked() {
                if filter == Some(Provider::Local) || available == [Provider::Local] {
                    app.rail_filter = crate::app::RailFilter::Local;
                }
                app.navigate(Route::Library);
            }
        }
        ui.add_space(16.0);
    });
}

fn identities(app: &App, connected: &[Provider]) -> Vec<Identity> {
    let mut identities = Vec::new();
    if connected.contains(&Provider::YouTubeMusic) {
        let selected = app
            .accounts
            .iter()
            .find(|account| account.page_id == app.settings.account_page_id)
            .or_else(|| app.accounts.iter().find(|account| account.selected))
            .or(app.accounts.first());
        let name = app
            .profile
            .youtube
            .as_ref()
            .map(|profile| profile.name.clone())
            .or_else(|| selected.map(|account| account.name.clone()))
            .or_else(|| match &app.auth {
                AuthState::SignedIn { account } => Some(account.clone()),
                _ => None,
            });
        if let Some(name) = name.filter(|name| !name.trim().is_empty()) {
            identities.push(Identity {
                provider: Provider::YouTubeMusic,
                name,
                thumbnail: app
                    .profile
                    .youtube
                    .as_ref()
                    .and_then(|profile| profile.thumbnail.clone())
                    .or_else(|| selected.and_then(|account| account.thumbnail.clone())),
                audience: app
                    .profile
                    .youtube
                    .as_ref()
                    .and_then(|profile| profile.subscribers_long.clone()),
                youtube_id: app
                    .profile
                    .youtube_id
                    .clone()
                    .or_else(|| app.own_channel_id().map(str::to_owned)),
                url: None,
            });
        }
    }
    if connected.contains(&Provider::Spotify)
        && let Some(profile) = &app.profile.spotify
    {
        identities.push(Identity {
            provider: Provider::Spotify,
            name: profile.name.clone(),
            thumbnail: profile.thumbnail.clone(),
            audience: profile.followers.map(|count| {
                format!(
                    "{} {}",
                    crate::spotify::format_count(count),
                    if count == 1 { "follower" } else { "followers" }
                )
            }),
            youtube_id: None,
            url: profile.url.as_deref().and_then(spotify_profile_url),
        });
    }
    identities
}

fn spotify_profile_url(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.host_str() == Some("open.spotify.com")
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url
            .path()
            .strip_prefix("/user/")
            .is_some_and(|id| !id.is_empty()))
    .then(|| url.to_string())
}

fn identity_header(app: &mut App, ui: &mut egui::Ui, identity: &Identity) {
    let top = ui.cursor().top();
    let out = super::page_header(
        ui,
        &app.palette,
        &mut app.images,
        &app.backend,
        super::PageHeader {
            source: Some(identity.provider.name()),
            art_url: identity.thumbnail.as_ref(),
            art_size: super::HEADER_ART_PT,
            circle: true,
            kind_label: identity.provider.name(),
            title: &identity.name,
            verification: None,
            description: None,
            artists: &[],
            second_plain: identity.audience.as_deref(),
            meta: None,
            owner: None,
            credit_air: 4.0,
            play_size: None,
            playing: false,
            save_in_library: None,
            dots: false,
            salt: "own-profile",
        },
    );
    ui.add_space(out.art_bottom - top + 12.0);
    if (identity.youtube_id.is_some() || identity.url.is_some())
        && widgets::outline_button(ui, &app.palette, "Open profile").clicked()
    {
        open_identity(app, identity);
    }
    ui.add_space(12.0);
}

fn identity_row(app: &mut App, ui: &mut egui::Ui, identity: &Identity) {
    let p = app.palette;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), theme::ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let art_rect = egui::Rect::from_center_size(
        rect.left_center() + egui::vec2(theme::ART_SIZE / 2.0, 0.0),
        egui::Vec2::splat(theme::ART_SIZE),
    );
    let art = super::art_texture(
        &mut app.images,
        &app.backend,
        ui.ctx(),
        identity.thumbnail.as_ref(),
        theme::ART_SIZE,
    );
    widgets::paint_art(ui, &p, art_rect, art, widgets::ArtShape::Circle);
    let text_x = art_rect.right() + 12.0;
    let action = identity.youtube_id.is_some() || identity.url.is_some();
    let action_width = if action {
        widgets::text_width(ui, "Open profile", &theme::semibold(13.0)) + 40.0
    } else {
        0.0
    };
    let text_width = (rect.right() - text_x - action_width).max(0.0);
    crate::emoji::paint_line(
        ui,
        egui::pos2(text_x, rect.center().y - 9.5),
        &widgets::truncate_to(
            ui,
            &identity.name,
            theme::medium(theme::ROW_TITLE_PT),
            text_width,
        ),
        theme::medium(theme::ROW_TITLE_PT),
        p.text,
    );
    let detail = identity.audience.as_ref().map_or_else(
        || identity.provider.name().to_owned(),
        |audience| format!("{} • {audience}", identity.provider.name()),
    );
    crate::emoji::paint_line(
        ui,
        egui::pos2(text_x, rect.center().y + 9.5),
        &widgets::truncate_to(ui, &detail, theme::regular(12.0), text_width),
        theme::regular(12.0),
        p.secondary,
    );
    if action {
        let mut action_ui = ui.new_child(
            egui::UiBuilder::new()
                .id(ui.id().with(("profile-link", identity.provider)))
                .max_rect(egui::Rect::from_min_max(
                    egui::pos2(rect.right() - action_width, rect.top()),
                    rect.max,
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        if widgets::outline_button(&mut action_ui, &p, "Open profile").clicked() {
            open_identity(app, identity);
        }
    }
}

fn open_identity(app: &mut App, identity: &Identity) {
    if let Some(id) = &identity.youtube_id {
        app.mark_profile(id);
        app.navigate(Route::Artist(id.clone()));
    } else if let Some(url) = &identity.url {
        crate::open_url(url);
    }
}

fn provider_status(app: &mut App, ui: &mut egui::Ui, provider: Provider) {
    let (loading, error) = match provider {
        Provider::YouTubeMusic => (
            app.profile.youtube_loading,
            app.profile.youtube_error.clone(),
        ),
        Provider::Spotify => (
            app.profile.spotify_loading,
            app.profile.spotify_error.clone(),
        ),
        Provider::Local => return,
    };
    if loading {
        ui.label(
            RichText::new(format!("Loading {} profile…", provider.name()))
                .font(theme::regular(12.0))
                .color(app.palette.secondary),
        );
    }
    if let Some(error) = error {
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{}: {error}", provider.name()))
                    .font(theme::regular(12.0))
                    .color(app.palette.danger),
            );
            if widgets::outline_button(ui, &app.palette, "Try again").clicked() {
                app.retry_profile();
            }
        });
    }
}

fn youtube_sections(app: &mut App, ui: &mut egui::Ui) {
    let Some(profile) = app.profile.youtube.as_ref() else {
        return;
    };
    let tracks: Vec<_> = profile
        .top_songs
        .iter()
        .take(PREVIEW_TRACKS)
        .cloned()
        .collect();
    let title = profile
        .top_songs_title
        .as_deref()
        .unwrap_or("Top songs")
        .to_owned();
    let shelves = Arc::clone(&profile.shelves);
    let id = app
        .profile
        .youtube_id
        .clone()
        .unwrap_or_else(|| profile.id.clone());
    if !tracks.is_empty() {
        super::section(ui, &app.palette, &format!("{title} · YouTube Music"));
        let signed_in = app.signed_in();
        let outcome = ui
            .push_id("profile-youtube-tracks", |ui| {
                super::track_rows(
                    ui,
                    &app.palette,
                    &mut app.images,
                    &app.backend,
                    &mut app.stats,
                    &app.liked,
                    &app.now,
                    &tracks,
                    false,
                    None,
                    &app.library.playlists.items,
                    signed_in,
                    false,
                    false,
                )
            })
            .inner;
        if let Some(action) = outcome.action {
            app.row_action(action);
        }
        match outcome.click {
            Some(super::ListClick::Row(index)) => app.start_from(tracks, index),
            Some(super::ListClick::Link(route)) => app.navigate(route),
            None => {}
        }
    }
    app.mark_profile(&id);
    for (index, shelf) in shelves.iter().enumerate() {
        ui.push_id(("profile-youtube-shelf", index), |ui| {
            super::shelf_carousel(
                app,
                ui,
                "own-profile-youtube",
                shelf,
                Some(Route::Artist(id.clone())),
                super::MoreButton::ShowAll,
            );
        });
    }
}

fn playlist_shelf(app: &mut App, ui: &mut egui::Ui, provider: Provider) {
    let source = match provider {
        Provider::YouTubeMusic => PlaylistSource::YouTube,
        Provider::Spotify => PlaylistSource::Spotify,
        Provider::Local => PlaylistSource::Local,
    };
    let items: Vec<_> = app
        .library
        .playlists
        .items
        .iter()
        .filter(|playlist| playlist.source == source)
        .take(PREVIEW_PLAYLISTS)
        .cloned()
        .map(Entry::Playlist)
        .collect();
    if items.is_empty() {
        return;
    }
    let shelf = Shelf {
        title: if provider == Provider::Local {
            "Local playlists"
        } else {
            "Saved playlists"
        }
        .to_owned(),
        strapline: Some(provider.name().to_owned()),
        items,
        more_params: None,
        more_browse_id: None,
        expanded: false,
    };
    ui.push_id(("profile-playlists", provider), |ui| {
        super::shelf_carousel(
            app,
            ui,
            "own-profile-library",
            &shelf,
            Some(Route::Library),
            super::MoreButton::Pill("Open library"),
        );
    });
}

fn saved_songs(app: &mut App, ui: &mut egui::Ui, available: &[Provider]) {
    let mut entries = Vec::new();
    for &provider in available {
        if app
            .profile
            .filter
            .is_some_and(|selected| selected != provider)
        {
            continue;
        }
        if provider == Provider::Local {
            entries.extend(
                app.local_tracks
                    .iter()
                    .take(PREVIEW_TRACKS)
                    .cloned()
                    .map(Entry::Song),
            );
        } else {
            entries.extend(
                app.library
                    .songs
                    .items
                    .iter()
                    .filter(|track| track.provider() == provider)
                    .take(PREVIEW_TRACKS)
                    .cloned()
                    .map(Entry::Song),
            );
        }
    }
    if entries.is_empty() {
        return;
    }
    let title = match app.profile.filter {
        Some(Provider::Local) => "Local songs",
        Some(_) => "Saved songs",
        None if available == [Provider::Local] => "Local songs",
        None => "Songs",
    };
    super::section(ui, &app.palette, title);
    if app.profile.entries != entries {
        app.profile.entries = entries;
        app.profile.revision = app.profile.revision.wrapping_add(1);
    }
    let mut rows = std::mem::take(&mut app.profile.rows);
    rows.set_entries(
        &app.profile.entries,
        app.profile.revision,
        app.can_group_sources(),
    );
    ui.push_id("profile-saved-songs", |ui| {
        super::source_rows::draw(
            app,
            ui,
            &mut rows,
            "own-profile-songs",
            super::source_rows::Playback::List,
        );
    });
    app.profile.rows = rows;
}
