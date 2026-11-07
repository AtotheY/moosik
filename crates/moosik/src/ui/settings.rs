//! Settings: a strip of tabs, and under it cards of rows for the open one.
//!
//! Sign-in lives on the Account tab. The primary path is importing cookies from a browser you are
//! already signed into. It is not a convenience: a cookie copied from the DevTools console
//! *cannot* work, because `document.cookie` is specified not to expose `HttpOnly` cookies and the
//! ones that authenticate a YouTube session — `HSID`, `SSID`, `LOGIN_INFO`, `__Secure-1PSID`,
//! `__Secure-3PSID` — are all HttpOnly. Pasting from the console produces a cookie that
//! `ytmapi-rs` accepts and YouTube ignores. The `Cookie` *request header* from the Network tab
//! does carry them, and that is what the Paste cookies window asks for (`ui::paste_cookies`).

use egui::RichText;

use crate::app::App;
use crate::auth::AuthState;
use crate::browser_cookies;
use crate::model::Provider;
use crate::settings::Opacity;
use crate::settings::{HomeSource, Quality, Theme};
use crate::theme;
use crate::widgets;

/// The tabs of the Settings page, in the order they are shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    Appearance,
    Playback,
    Providers,
    System,
    About,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 5] = [
        SettingsTab::Appearance,
        SettingsTab::Playback,
        SettingsTab::Providers,
        SettingsTab::System,
        SettingsTab::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::Appearance => "Appearance",
            SettingsTab::Playback => "Playback",
            SettingsTab::Providers => "Providers",
            SettingsTab::System => "System",
            SettingsTab::About => "About",
        }
    }
}

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    // The title scrolls away with the page rather than staying pinned over it.
    crate::ui::page_scroll(app, "settings").show(ui, |ui| {
        let mut column = ui.available_rect_before_wrap();
        let width = column.width().min(COLUMN_MAX_W);
        column.min.x += (column.width() - width) / 2.0;
        column.max.x = column.min.x + width;
        ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
            ui.set_width(width);
            crate::ui::page_title(ui, &app.palette, "Settings");

            ui.horizontal_wrapped(|ui| {
                for tab in SettingsTab::ALL {
                    if widgets::chip(ui, &app.palette, tab.label(), app.settings_tab == tab)
                        .clicked()
                    {
                        app.settings_tab = tab;
                    }
                }
            });
            ui.add_space(20.0);

            match app.settings_tab {
                SettingsTab::Appearance => appearance(app, ui),
                SettingsTab::Playback => playback(app, ui),
                SettingsTab::Providers => providers(app, ui),
                SettingsTab::System => system(app, ui),
                SettingsTab::About => about(app, ui),
            }
            ui.add_space(16.0);
        });
    });
}

fn providers(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        for provider in Provider::ALL {
            if widgets::chip(
                ui,
                &app.palette,
                provider.name(),
                app.settings_provider == provider,
            )
            .clicked()
            {
                app.settings_provider = provider;
            }
        }
    });
    ui.add_space(20.0);
    let available = app.available_providers();
    if available.len() > 1 {
        let p = app.palette;
        let mut preferred = if available.contains(&app.settings.preferred_provider) {
            app.settings.preferred_provider
        } else {
            available[0]
        };
        let mut changed = false;
        widgets::setting_group(ui, &p, None, |ui, g| {
            g.wide_row(
                ui,
                "Preferred source",
                "Default for matching songs and profiles.",
                |ui| {
                    for &provider in &available {
                        if widgets::chip(ui, &p, provider.name(), preferred == provider).clicked() {
                            preferred = provider;
                            changed = true;
                        }
                    }
                },
            );
        });
        if changed {
            let ctx = ui.ctx().clone();
            app.update_settings(&ctx, |s| s.preferred_provider = preferred);
        }
    }
    match app.settings_provider {
        Provider::YouTubeMusic => account(app, ui),
        Provider::Spotify => spotify(app, ui),
        Provider::Local => library(app, ui),
    }
}

/// The About tab: the shortcuts window, then the app's own name, version and licence.
fn about(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;
    let mut open_help = false;
    widgets::setting_group(ui, &p, Some("Help"), |ui, g| {
        g.row(
            ui,
            "Keyboard shortcuts",
            "Every key Moosik answers to.",
            |ui| open_help = widgets::outline_button(ui, &p, "Show").clicked(),
        );
    });
    if open_help {
        app.help_open = true;
    }

    // Updates: the launch check's switch, and what the last check found.
    let mut check = app.settings.check_for_updates;
    #[cfg(target_os = "macos")]
    if let Some(native_check) = crate::update::macos::automatic_checks() {
        check = native_check;
    }
    let before_check = check;
    let (mut check_now, mut show) = (false, false);
    let (title, detail) = match &app.update.available {
        Some(release) => (
            format!("Moosik {} is available", release.version),
            format!("You have {}.", crate::update::current_version()),
        ),
        None if app.update.checking => (
            "Checking for updates…".to_owned(),
            "Asking GitHub for the latest release.".to_owned(),
        ),
        None if app.update.not_published => (
            "Updates aren’t published yet".to_owned(),
            "Check back when the first Moosik update is ready.".to_owned(),
        ),
        None if app.update.error.is_some() => (
            "Couldn’t check for updates".to_owned(),
            "Please try again later.".to_owned(),
        ),
        None if app.update.up_to_date => (
            "Moosik is up to date".to_owned(),
            format!(
                "No newer Moosik release than {} is available.",
                env!("CARGO_PKG_VERSION")
            ),
        ),
        None => (
            "Latest version".to_owned(),
            "Asks GitHub for the latest release.".to_owned(),
        ),
    };
    widgets::setting_group(ui, &p, Some("Updates"), |ui, g| {
        toggle_row(
            g,
            ui,
            &p,
            "setting-check-updates",
            "Check for updates",
            "Shows a small update icon when a new Moosik release is available.",
            &mut check,
        );
        g.row(ui, &title, &detail, |ui| {
            if app.update.available.is_some() {
                show = widgets::outline_button(ui, &p, "Update").clicked();
            } else if !app.update.checking {
                check_now = widgets::outline_button(ui, &p, "Check now").clicked();
            }
        });
    });
    let ctx = ui.ctx().clone();
    if check != before_check {
        #[cfg(target_os = "macos")]
        crate::update::macos::set_automatic_checks(check);
        app.update_settings(&ctx, |s| s.check_for_updates = check);
    }
    if check_now {
        app.check_for_update(true);
    }
    if show {
        app.activate_update();
    }

    if app.logo.is_none() {
        app.logo = crate::logo_texture(ui.ctx());
    }
    let logo = app.logo.clone();
    widgets::setting_group(ui, &p, Some("About"), |ui, g| {
        // An open-source app's own about-ish footer: icon, name, version, licence.
        g.free(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(logo) = &logo {
                    ui.add(egui::Image::new((logo.id(), egui::Vec2::splat(32.0))));
                    ui.add_space(8.0);
                }
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("Moosik")
                            .font(theme::semibold(14.0))
                            .color(p.text),
                    );
                    ui.label(
                        RichText::new(format!("v{} · GPL-3.0-or-later", env!("CARGO_PKG_VERSION")))
                            .font(theme::regular(11.0))
                            .color(p.dim),
                    );
                });
            });
        });
    });
}

/// Why the last sign-in attempt failed, shown wherever the user is.
fn sign_in_error(app: &App, ui: &mut egui::Ui) {
    let Some(msg) = &app.sign_in_error else {
        return;
    };
    ui.label(
        RichText::new(msg)
            .font(theme::regular(12.0))
            .color(app.palette.danger),
    );
    ui.add_space(10.0);
}

/// What needs saying under the Scheme row: the painting theme's warnings, a chosen theme that
/// is gone, the built-in stand-in when the folder has nothing of this mode, and files that
/// failed to read.
fn theme_notes(app: &App, ui: &mut egui::Ui, dark: bool) {
    let themes = &app.custom_themes;
    let mode = if dark { "dark" } else { "light" };
    let chosen = if dark {
        &app.settings.dark_theme
    } else {
        &app.settings.light_theme
    };
    match app.settings.painting_theme(dark, themes) {
        Some(t) => {
            if t.name != *chosen {
                problem(
                    app,
                    ui,
                    &format!("“{chosen}” is missing, using “{}”.", t.name),
                );
            }
            for warning in &t.warnings {
                note(app, ui, warning);
            }
        }
        None => problem(
            app,
            ui,
            &format!("No {mode} themes in the folder, using Classic."),
        ),
    }
    for (file, error) in &themes.errors {
        problem(app, ui, &format!("{file}: {error}"));
    }
}

/// A [`note`] that reports something wrong.
fn problem(app: &App, ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::regular(11.0))
            .color(app.palette.danger),
    );
}

fn note(app: &App, ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::regular(11.0))
            .color(app.palette.dim),
    );
}

/// How wide the settings column gets. Past this the switches, right-aligned, drifted a window's
/// width away from the words that name them.
const COLUMN_MAX_W: f32 = 1040.0;

/// A titled row with a persisted on/off switch.
fn toggle_row(
    g: &mut widgets::Group,
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    id: &'static str,
    title: &str,
    detail: &str,
    value: &mut bool,
) {
    g.row(ui, title, detail, |ui| {
        widgets::toggle(ui, palette, ui.id().with(id), value);
    });
}

fn appearance(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;
    app_icon_picker(app, ui);
    home_settings(app, ui);

    let mut theme_choice = app.settings.theme;
    // The mode on screen decides which themes are offered: a theme file says whether it is
    // dark or light, and picking one sets the theme for that mode only, so switching modes
    // (or following the desktop) swaps between two remembered choices.
    let dark = app.palette.dark;
    let mut dark_choice = app.settings.dark_theme.clone();
    let mut light_choice = app.settings.light_theme.clone();
    let offered: Vec<&crate::theme::custom::CustomTheme> = app
        .custom_themes
        .themes
        .iter()
        .filter(|t| t.dark == dark)
        .collect();
    let options: Vec<(egui::Color32, &str)> = offered
        .iter()
        .map(|t| (t.palette().accent, t.name.as_str()))
        .collect();
    let painting = app.settings.painting_theme(dark, &app.custom_themes);
    let (current_swatch, current_label) = match painting {
        Some(t) => (t.palette().accent, t.name.as_str()),
        None => (app.palette.accent, "Classic (built-in)"),
    };

    widgets::setting_group(ui, &p, Some("Colours"), |ui, g| {
        // Chips are laid out right to left, so the last option goes in first.
        g.row(
            ui,
            "Mode",
            "Follow the desktop, or stay dark or light.",
            |ui| {
                for option in Theme::ALL.into_iter().rev() {
                    if widgets::chip(ui, &p, option.label(), theme_choice == option).clicked() {
                        theme_choice = option;
                    }
                }
            },
        );
        g.row(ui, "Theme", "The colours used in this mode.", |ui| {
            if let Some(i) = widgets::dropdown(
                ui,
                &p,
                ui.id().with("scheme-dropdown"),
                current_swatch,
                current_label,
                &options,
            ) {
                let name = offered[i].name.clone();
                if dark {
                    dark_choice = name;
                } else {
                    light_choice = name;
                }
            }
        });
        g.row(
            ui,
            "Your own themes",
            "Drop a theme file in the folder and it appears above.",
            |ui| {
                if widgets::outline_button(ui, &p, "Open folder").clicked() {
                    match crate::theme::custom::dir() {
                        Some(dir) => {
                            if let Err(e) = std::fs::create_dir_all(&dir) {
                                tracing::warn!(%e, "could not create the themes folder");
                            }
                            crate::open_url(&dir.to_string_lossy());
                        }
                        None => tracing::warn!("no config directory for the themes folder"),
                    }
                }
            },
        );
    });
    theme_notes(app, ui, dark);

    let mut adaptive_art_color = app.settings.adaptive_art_color;
    let mut sheer = app.settings.opacity.sheer();
    let mut blur = app.settings.blur;
    let mut card_outline = app.settings.card_outline;
    let mut scroll_fades = app.settings.scroll_fades;
    widgets::setting_group(ui, &p, Some("Window"), |ui, g| {
        // A slider, not chips: transparency is a continuum, and four names only ever made
        // the setting look like a choice of four windows.
        let decorated = app.decorated;
        let percent = format!("{:.0}%", sheer / Opacity::MAX * 100.0);
        g.row(
            ui,
            "Transparency",
            if decorated {
                "Not available with `--decorated`."
            } else {
                "How much of the desktop shows through."
            },
            |ui| {
                // `--decorated` gives the window back to the OS, which paints its own opaque
                // background — the slider would do nothing, so it is dimmed and inert rather
                // than silently pretending.
                ui.add_enabled_ui(!decorated, |ui| {
                    // The number the slider only implies, in a fixed slot at the row's right
                    // edge so the changing digits move nothing: a percentage of the
                    // slider's whole range, which reaches past the old maximum — not of the
                    // composite alpha.
                    ui.allocate_ui_with_layout(
                        egui::vec2(34.0, 20.0),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(&percent)
                                    .font(theme::regular(12.0))
                                    .color(p.text),
                            );
                        },
                    );
                    ui.add_space(4.0);
                    widgets::slider(ui, &p, &mut sheer, 0.0..=Opacity::MAX, 150.0);
                });
            },
        );

        let blur_available = app.blur.available();
        let blur_useful = sheer > 0.0;
        g.row(
            ui,
            "Background blur",
            if !blur_available {
                if cfg!(target_os = "windows") {
                    "Needs Windows 11 22H2 or later."
                } else {
                    "Your compositor doesn't support it."
                }
            } else if !blur_useful {
                "Turn up transparency to use it."
            } else {
                "Blurs the desktop behind the window."
            },
            |ui| {
                // Dimmed and inert rather than clickable-but-pointless when there is
                // nothing to turn on — no compositor support, or nothing set to show
                // through it yet.
                widgets::toggle_enabled(
                    ui,
                    &p,
                    ui.id().with("setting-blur"),
                    &mut blur,
                    blur_available && blur_useful,
                );
            },
        );
        toggle_row(
            g,
            ui,
            &p,
            "setting-card-outline",
            "Card outlines",
            "A thin edge around each panel.",
            &mut card_outline,
        );
        toggle_row(
            g,
            ui,
            &p,
            "setting-scroll-fades",
            "Scroll fades",
            "Fades lists out at their edges. Recommended with transparency.",
            &mut scroll_fades,
        );
        toggle_row(
            g,
            ui,
            &p,
            "setting-adaptive-art-color",
            "Adaptive header colour",
            "Tints album and playlist headers to match the cover.",
            &mut adaptive_art_color,
        );
    });

    let ctx = ui.ctx().clone();
    app.update_settings(&ctx, |s| {
        s.opacity = Opacity::new(sheer);
        s.blur = blur;
        s.theme = theme_choice;
        s.dark_theme = dark_choice;
        s.light_theme = light_choice;
        s.adaptive_art_color = adaptive_art_color;
        s.card_outline = card_outline;
        s.scroll_fades = scroll_fades;
    });
}

fn playback(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;
    let mut keep_order = app.settings.queue_click_keeps_order;
    let mut double_click = app.settings.double_click_to_play;
    let mut fullscreen_lyrics = app.settings.fullscreen_lyrics;

    widgets::setting_group(ui, &p, Some("Player"), |ui, g| {
        toggle_row(
            g,
            ui,
            &p,
            "setting-double-click",
            "Double-click to play",
            "One click selects, two clicks play.",
            &mut double_click,
        );
        toggle_row(
            g,
            ui,
            &p,
            "setting-fullscreen-lyrics",
            "Lyrics in fullscreen",
            "Shows lyrics beside the cover when available.",
            &mut fullscreen_lyrics,
        );
    });

    widgets::setting_group(ui, &p, Some("Queue"), |ui, g| {
        toggle_row(
            g,
            ui,
            &p,
            "setting-queue-keep-order",
            "Keep the songs you jump over",
            "Playing a song further down Up next plays it now and keeps the songs before it.",
            &mut keep_order,
        );
    });

    let ctx = ui.ctx().clone();
    app.update_settings(&ctx, |s| {
        s.queue_click_keeps_order = keep_order;
        s.double_click_to_play = double_click;
        s.fullscreen_lyrics = fullscreen_lyrics;
    });
}

fn library(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;
    let mut playlist_sort = app.settings.playlist_default_sort;

    widgets::setting_group(ui, &p, Some("Songs and playlists"), |ui, g| {
        g.wide_row(
            ui,
            "Playlist order",
            "How playlists are ordered when you open them. The sort button on a playlist still changes it for that visit.",
            |ui| {
                for option in crate::app::PlaylistSort::ALL {
                    let label = match option {
                        // Named for what it shows in a playlist you add to: the latest songs on
                        // top.
                        crate::app::PlaylistSort::Default => "As saved",
                        crate::app::PlaylistSort::Reverse => "Newest added first",
                        other => other.label(),
                    };
                    if widgets::chip(ui, &p, label, playlist_sort == option).clicked() {
                        playlist_sort = option;
                    }
                }
            },
        );
    });

    // Which sections come first on Home: a window of its own, since it is a list to drag about.
    let mut strict = app.settings.home_strict_order;
    widgets::setting_group(ui, &p, Some("Home"), |ui, g| {
        g.row(
            ui,
            "Order of homepage",
            "Choose which sections come first on Home.",
            |ui| {
                open_order = widgets::outline_button(ui, &p, "Change…").clicked();
            },
        );
        toggle_row(
            g,
            ui,
            &p,
            "setting-home-strict",
            "Strict Home order",
            "Waits longer for Home so your top sections come first. Gives up after a few seconds.",
            &mut strict,
        );
    });
    if open_order {
        app.home_order_open = true;
    }

    let ctx = ui.ctx().clone();
    app.update_settings(&ctx, |s| {
        s.playlist_default_sort = playlist_sort;
    });

    local_music(app, ui);
}

/// Folders scanned for local audio files (mp3, wav, flac, and more), shown in the Library's
/// Local tab. Adding or removing a folder here is what triggers the rescan — see
/// `App::update_settings`'s own diff branch for `local_folders`.
fn local_music(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;
    let mut folders = app.settings.local_folders.clone();
    let mut removed = None;
    let mut add = false;
    widgets::setting_group(ui, &p, Some("Local files"), |ui, g| {
        for (i, folder) in folders.iter().enumerate() {
            g.row(ui, folder, "", |ui| {
                if widgets::icon_button(ui, &p, crate::icons::Icon::X, 12.0, false).clicked() {
                    removed = Some(i);
                }
            });
        }
        g.row(
            ui,
            "Add a folder",
            "Folders to scan for music. They show up in Library → Local.",
            |ui| {
                add = widgets::outline_button(ui, &p, "Add folder").clicked();
            },
        );
    });
    if let Some(i) = removed {
        folders.remove(i);
    }
    if add && let Some(dir) = rfd::FileDialog::new().pick_folder() {
        let path = dir.to_string_lossy().into_owned();
        if !folders.contains(&path) {
            folders.push(path);
        }
    }

    let ctx = ui.ctx().clone();
    app.update_settings(&ctx, |s| {
        s.local_folders = folders;
    });
}

fn system(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;

    // Every platform this ships to has a mechanism — an XDG autostart entry on Linux, a
    // LaunchAgent on macOS, the per-user `Run` key on Windows (`autostart`'s own doc comment) —
    // so the row exists wherever one does, and nowhere else.
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    let mut start_on_boot = app.settings.start_on_boot;
    let mut hide_tray = app.settings.hide_tray_on_close;
    // Wayland has no way to hide a window and show it again from code — only a real minimize,
    // and only in the hide direction (`app::hide_command`'s own doc comment has the specifics)
    // — so the promise made here has to be the one Wayland can actually keep, or the toggle
    // reads as broken the first time someone clicks the tray's Show and nothing happens.
    let hide_tray_detail = if crate::app::is_wayland() {
        "Closing minimises instead of quitting. Quit from the tray."
    } else {
        "Closing hides to the tray instead of quitting."
    };
    widgets::setting_group(ui, &p, Some("Desktop"), |ui, g| {
        // Not inside a Flatpak: its autostart entry would land in the sandbox's own config
        // folder, where nothing reads it.
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        if !crate::flatpak::sandboxed() {
            toggle_row(
                g,
                ui,
                &p,
                "setting-start-on-boot",
                "Launch on startup",
                "Opens Moosik when you log in.",
                &mut start_on_boot,
            );
        }
        toggle_row(
            g,
            ui,
            &p,
            "setting-hide-tray",
            "Close to tray",
            hide_tray_detail,
            &mut hide_tray,
        );
    });

    #[cfg(target_os = "macos")]
    mac_media_fallback(app, ui);

    // Discord Rich Presence. Nothing to configure: the app connects as its own Discord
    // application (the id is fixed in `crate::discord`), so the toggle is the whole setting.
    let mut discord = app.settings.discord_rich_presence;
    let mut hosting = app.hosting();
    let mut leave = false;
    widgets::setting_group(ui, &p, Some("Integrations"), |ui, g| {
        toggle_row(
            g,
            ui,
            &p,
            "setting-discord",
            "Discord Rich Presence",
            "Shows what you're playing on Discord.",
            &mut discord,
        );
        if app.following() {
            g.row(
                ui,
                "Listen along",
                "Following the host’s playback.",
                |ui| {
                    leave = widgets::outline_button(ui, &p, "Leave").clicked();
                },
            );
        } else if discord {
            toggle_row(
                g,
                ui,
                &p,
                "host-listen-along",
                "Host Listen along",
                "Let friends on Discord follow your playback for this session.",
                &mut hosting,
            );
        }
    });

    let ctx = ui.ctx().clone();
    app.update_settings(&ctx, |s| {
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        {
            s.start_on_boot = start_on_boot;
        }
        s.hide_tray_on_close = hide_tray;
        s.discord_rich_presence = discord;
    });
    if leave || (app.hosting() && !hosting) {
        app.leave_room();
    } else if discord && hosting && !app.hosting() {
        app.start_hosting();
    }
}

#[cfg(target_os = "macos")]
fn mac_media_fallback(app: &mut App, ui: &mut egui::Ui) {
    use crate::media_fallback::Status;
    let p = app.palette;
    let state = &mut app.media_fallback;
    state.refresh(ui.ctx());
    let mut enabled = matches!(state.status, Some(Status::On | Status::Approval));
    let before = enabled;
    let mut open_settings = false;
    widgets::setting_group(ui, &p, Some("Media keys"), |ui, g| {
        g.row(
            ui,
            "Use Moosik instead of Apple Music",
            "Resumes Moosik when your Mac tries to open Apple Music.",
            |ui| {
                widgets::toggle_enabled(
                    ui,
                    &p,
                    ui.id().with("media-fallback"),
                    &mut enabled,
                    !state.busy()
                        && matches!(
                            state.status,
                            Some(Status::Off | Status::On | Status::Approval)
                        ),
                );
            },
        );
        g.free(ui, |ui| {
            ui.label(RichText::new("Works even when Moosik is closed. Other apps keep their media controls. Turn off to open Apple Music manually.")
                .font(theme::regular(11.0)).color(p.dim));
            if state.status == Some(Status::Approval) {
                ui.add_space(8.0);
                ui.label(RichText::new("Allow Moosik’s background activity in System Settings to finish enabling.")
                    .font(theme::regular(11.0)).color(p.secondary));
                ui.add_space(8.0);
                open_settings = widgets::outline_button(ui, &p, "Open System Settings").clicked();
            }
            if let Some(error) = &state.error {
                ui.add_space(8.0);
                ui.label(RichText::new(error).font(theme::regular(11.0)).color(p.secondary));
            }
        });
    });
    if enabled != before {
        state.start(ui.ctx(), if enabled { "--enable" } else { "--disable" });
    } else if open_settings {
        state.start(ui.ctx(), "--settings");
    }
}

fn account(app: &mut App, ui: &mut egui::Ui) {
    sign_in_error(app, ui);
    match &app.auth {
        AuthState::SignedIn { .. } => signed_in(app, ui),
        AuthState::Checking => {
            ui.label(
                RichText::new("Checking your saved session…")
                    .font(theme::regular(13.0))
                    .color(app.palette.secondary),
            );
        }
        _ => sign_in(app, ui),
    }
}

fn sign_in(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;
    if let AuthState::Failed(msg) = &app.auth {
        ui.label(
            RichText::new(msg)
                .font(theme::regular(13.0))
                .color(p.danger),
        );
        ui.add_space(10.0);
    }

    ui.add_enabled_ui(!app.sign_in_busy, |ui| {
        widgets::setting_group(ui, &p, Some("Sign in to YouTube Music"), |ui, g| {
            let browsers = app
                .detected_browsers
                .get_or_insert_with(|| {
                    if crate::flatpak::sandboxed() {
                        Vec::new()
                    } else {
                        browser_cookies::detected()
                    }
                })
                .clone();
            let has_browsers = !browsers.is_empty();
            if has_browsers {
                browser_logins(app, ui, g, browsers);
            }
            #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
            g.wide_row(
                ui,
                "Sign in with Google",
                "Opens a separate sign-in window.",
                |ui| {
                    let button = if !has_browsers {
                        widgets::pill_button(ui, &p, "Open sign-in window")
                    } else {
                        widgets::outline_button(ui, &p, "Open sign-in window")
                    };
                    if button.clicked() {
                        app.login_browser = None;
                        app.sign_in_error = None;
                        app.sign_in_busy = true;
                        app.backend.sign_in_from_window();
                    }
                },
            );
            paste_cookies(app, ui, g);
        });
    });
}

fn browser_logins(
    app: &mut App,
    ui: &mut egui::Ui,
    g: &mut widgets::Group,
    browsers: Vec<browser_cookies::Browser>,
) {
    let p = app.palette;
    g.wide_row(
        ui,
        "Use a browser login",
        "Choose the browser you use for YouTube Music.",
        |ui| {
            for (i, browser) in browsers.into_iter().enumerate() {
                let label = if app.sign_in_busy && app.login_browser.as_ref() == Some(&browser) {
                    format!("Checking {}…", browser.label)
                } else {
                    format!("Use my {} login", browser.label)
                };
                let button = if i == 0 {
                    widgets::pill_button(ui, &p, &label)
                } else {
                    widgets::outline_button(ui, &p, &label)
                };
                if button.clicked() {
                    app.import_from(browser);
                }
            }
        },
    );
    if let Some(browser) = &app.login_browser
        && app.sign_in_error.is_some()
    {
        let label = browser.label;
        g.wide_row(
            ui,
            &format!("Sign in to YouTube Music in {label}"),
            "Then return here and choose the same login again.",
            |ui| {
                if widgets::outline_button(ui, &p, &format!("Open {label}")).clicked()
                    && let Err(error) = browser.open_music()
                {
                    tracing::warn!(%error, label, "could not launch the selected browser");
                    app.sign_in_error = Some(format!(
                        "Couldn’t open {label}. Open music.youtube.com in {label} yourself."
                    ));
                }
            },
        );
    }
}

/// The way in when neither of the others can be used: the cookie header from a browser's
/// developer tools, pasted into [`crate::ui::paste_cookies`].
fn paste_cookies(app: &mut App, ui: &mut egui::Ui, g: &mut widgets::Group) {
    let p = app.palette;
    let mut clicked = false;
    g.row(
        ui,
        "Or paste cookies",
        "Copies the session from a browser's developer tools.",
        |ui| clicked = widgets::outline_button(ui, &p, "Paste").clicked(),
    );
    if clicked {
        app.paste_cookies_open = true;
    }
}

fn signed_in(app: &mut App, ui: &mut egui::Ui) {
    let p = app.palette;
    let account = match &app.auth {
        AuthState::SignedIn { account } => account.clone(),
        _ => String::new(),
    };
    let premium = app.premium;
    let mut sign_out = false;
    widgets::setting_group(ui, &p, Some("Account"), |ui, g| {
        // Naming the account is the point: it is YouTube confirming the session, not us
        // assuming it.
        g.row(
            ui,
            &if account.is_empty() {
                "Signed in".to_owned()
            } else {
                format!("Signed in as {account}")
            },
            "Your library and Home feed are synced.",
            |ui| sign_out = widgets::outline_button(ui, &p, "Sign out").clicked(),
        );
        // The Premium badge rides its own row: downloads follow it, so it should be seen.
        g.row(
            ui,
            "YouTube Premium",
            if premium {
                "Downloads are on."
            } else {
                "Downloads need YouTube Premium."
            },
            |ui| {
                if premium {
                    premium_badge(ui, &p);
                }
            },
        );
    });
    if sign_out {
        app.backend.sign_out();
    }
}

/// "Active": a small accent-tinted pill, painted by hand so a tall row cannot stretch it.
fn premium_badge(ui: &mut egui::Ui, palette: &crate::theme::Palette) {
    let galley = ui.fonts_mut(|f| {
        f.layout_no_wrap(
            "Active".to_owned(),
            theme::semibold(11.0),
            egui::Color32::PLACEHOLDER,
        )
    });
    let size = galley.size() + egui::vec2(18.0, 6.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(255),
        palette.accent.gamma_multiply(0.16),
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, palette.accent);
}

/// Native icon choice; textures are decoded once and retained in egui's context cache.
fn app_icon_picker(app: &mut App, ui: &mut egui::Ui) {
    let chosen = icon_grid(ui, &app.palette, app.settings.app_icon);
    let ctx = ui.ctx().clone();
    app.update_settings(&ctx, |s| s.app_icon = chosen);
}

fn icon_grid(
    ui: &mut egui::Ui,
    palette: &crate::theme::Palette,
    mut chosen: crate::app_icon::AppIcon,
) -> crate::app_icon::AppIcon {
    use crate::app_icon::AppIcon;
    ui.label(egui::RichText::new("App icon").size(18.0).strong());
    ui.label("A different finish. The same Moosik.");
    ui.add_space(12.0);
    let columns = ((ui.available_width() / 118.0).floor() as usize).clamp(2, 5);
    for row in AppIcon::ALL.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            for &icon in row {
                ui.push_id(icon.label(), |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(108.0);
                        let selected = chosen == icon;
                        if let Some(texture) = icon.texture(ui.ctx()) {
                            let image = egui::Image::new((texture.id(), egui::vec2(86.0, 86.0)));
                            let response = ui.add(
                                egui::Button::new(image)
                                    .selected(selected)
                                    .min_size(egui::vec2(108.0, 100.0))
                                    .corner_radius(12),
                            );
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::RadioButton,
                                    true,
                                    selected,
                                    format!("{} app icon", icon.label()),
                                )
                            });
                            if response
                                .on_hover_text(format!("Use {}", icon.label()))
                                .clicked()
                            {
                                chosen = icon;
                            }
                        }
                        ui.label(egui::RichText::new(icon.label()).size(12.0));
                        ui.label(
                            egui::RichText::new(if selected {
                                "Selected"
                            } else if icon == AppIcon::Ceramic {
                                "Default"
                            } else {
                                " "
                            })
                            .size(11.0)
                            .color(if selected {
                                palette.accent
                            } else {
                                palette.secondary
                            }),
                        );
                    });
                });
            }
        });
        ui.add_space(10.0);
    }
    ui.add_space(8.0);
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_cache_limit_saves_valid_edits_without_normalizing_the_input() {
        let mut limit = 2048;
        let mut draft = CacheLimitDraft::new(limit);
        draft.text = "004096".to_owned();
        draft.update(&mut limit, &(1..=1_048_576));
        assert_eq!(limit, 4096);
        assert_eq!(draft.applied, limit);
        assert_eq!(draft.text, "004096");
        assert!(draft.error.is_none());
    }

    #[test]
    fn custom_cache_limit_keeps_the_saved_value_while_an_edit_is_invalid() {
        let mut limit = 64;
        let mut draft = CacheLimitDraft::new(limit);
        for text in ["", "1", "12", "129", "18446744073709551616", "abc"] {
            draft.text = text.to_owned();
            draft.update(&mut limit, &(16..=128));
            assert_eq!(limit, 64);
            assert_eq!(draft.applied, 64);
            assert_eq!(draft.text, text);
            assert!(draft.error.is_some());
        }
        draft.text = "128".to_owned();
        draft.update(&mut limit, &(16..=128));
        assert_eq!(limit, 128);
        assert_eq!(draft.applied, 128);
        assert!(draft.error.is_none());
    }

    #[test]
    fn every_tab_has_its_own_label_and_the_first_is_the_default() {
        let labels: Vec<&str> = SettingsTab::ALL.iter().map(|t| t.label()).collect();
        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), SettingsTab::ALL.len());
        assert_eq!(SettingsTab::default(), SettingsTab::ALL[0]);
        assert_eq!(
            labels,
            ["Appearance", "Playback", "Providers", "System", "About"]
        );
    }
}
