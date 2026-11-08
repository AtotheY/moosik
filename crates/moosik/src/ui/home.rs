use std::sync::{Arc, Weak};

use crate::app::{App, Route};
use crate::home_feed::{self, DisplayShelf};
use crate::icons::Icon;
use crate::model::{HomeSection, Provider, Shelf};
use crate::settings::HomeSource;
use crate::theme;
use crate::ui::{failed, shelf_carousel, song_shelf};
use crate::widgets;

use super::source_rows;

struct CombinedDisplay {
    source: Weak<Vec<Shelf>>,
    primary: Option<Provider>,
    shelves: Vec<DisplayShelf>,
    song_rows: Vec<Option<source_rows::State>>,
}

fn combined_display(
    ui: &egui::Ui,
    shelves: &Arc<Vec<Shelf>>,
    primary: Option<Provider>,
) -> Arc<CombinedDisplay> {
    let id = ui.id().with("combined-home");
    let source = Arc::downgrade(shelves);
    if let Some(display) = ui.data(|data| data.get_temp::<Arc<CombinedDisplay>>(id))
        && display.source.ptr_eq(&source)
        && display.primary == primary
    {
        return display;
    }
    let shelves = home_feed::merge_categories(shelves, primary);
    let song_rows = shelves
        .iter()
        .map(|shelf| match shelf {
            DisplayShelf::Merged(shelf)
                if shelf.items.iter().all(|item| item.track().is_some()) =>
            {
                let mut rows = source_rows::State::default();
                rows.set_entries(&shelf.items, 0, true);
                Some(rows)
            }
            _ => None,
        })
        .collect();
    let display = Arc::new(CombinedDisplay {
        source,
        primary,
        shelves,
        song_rows,
    });
    ui.data_mut(|data| data.insert_temp(id, Arc::clone(&display)));
    display
}

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    // No "Home" heading: the sidebar already says where you are.
    if app.home_loading && app.home.is_none() {
        // The first row's distance from the card's top, matching the sides.
        ui.add_space(theme::PAGE_INSET_X);
        // The feed's own mood chips head it, as `filter_chips` draws them.
        filter_chips(app, ui);
        ui.add_space(4.0);
        // The real feed is several titled shelves, mixing grids and row-lists — never one
        // untitled block. Which shelves are coming can't be known before they load, so this is
        // a reasonable approximation of the shape, not a prediction of the exact content.
        // In the order the feed usually opens with: Listen again's cards, Quick picks' song
        // grid, then more cards. Each with the air `home_shelf` and `song_shelf` give theirs.
        ui.add_space(20.0);
        // Signed in, Listen again carries a line over its title — the account's name.
        if app.home_sources().youtube && app.signed_in() {
            widgets::skeleton_strapline(ui, &app.palette);
        }
        widgets::skeleton_shelf(
            ui,
            &app.palette,
            widgets::SkeletonShelfShape::Cards(widgets::ArtShape::Square),
            1,
        );
        widgets::skeleton_shelf(
            ui,
            &app.palette,
            widgets::SkeletonShelfShape::SongGrid,
            crate::ui::HOME_SONG_ROWS,
        );
        ui.add_space(20.0);
        widgets::skeleton_shelf(
            ui,
            &app.palette,
            widgets::SkeletonShelfShape::Cards(widgets::ArtShape::Square),
            1,
        );
        return;
    }
    if let Some(err) = app.home_error.clone() {
        failed(app, ui, err, App::retry_route);
        return;
    }

    crate::ui::page_scroll(app, "home").show(ui, |ui| {
        // Inside the scroll content, not before the scroll area: this is the first row's own
        // distance from the card's top — the same as from its sides — and it scrolls away with
        // the feed. Outside the area it was a fixed band the feed was clipped under.
        ui.add_space(theme::PAGE_INSET_X);
        home_warnings(app, ui);
        sign_in_note(app, ui);
        filter_chips(app, ui);

        // One snapshot for the frame — a refcount bump, not a copy of every shelf. `shelf`
        // needs `&mut App` for navigation and the texture cache, which rules out borrowing
        // the feed out of `app` across the call.
        let Some(shelves) = app.home.clone() else {
            return;
        };
        if shelves.iter().all(|shelf| shelf.items.is_empty()) {
            if app.home_has_more() || app.home_more_loading {
                app.load_home_more();
                widgets::skeleton_shelf(
                    ui,
                    &app.palette,
                    widgets::SkeletonShelfShape::Cards(widgets::ArtShape::Square),
                    1,
                );
            } else {
                empty_home(app, ui);
            }
            return;
        }
        let display = (app.effective_home_source() == HomeSource::Combined
            && app.home_filter.is_none())
        .then(|| combined_display(ui, &shelves, app.settings.home_primary_provider));
        for (index, shelf) in shelves.iter().enumerate() {
            let projected = display
                .as_ref()
                .map(|display| (&display.shelves[index], display.song_rows[index].as_ref()));
            let source = shelf
                .items
                .first()
                .map(crate::source_groups::entry_provider);
            match projected {
                Some((DisplayShelf::Hidden, _)) => {}
                Some((DisplayShelf::Merged(shelf), rows)) => {
                    ui.push_id(("home-merged-shelf", index), |ui| {
                        merged_shelf(app, ui, shelf, rows);
                    });
                }
                Some((DisplayShelf::Native, _)) | None => {
                    ui.push_id(("home-shelf", source, index), |ui| {
                        home_shelf(app, ui, shelf)
                    });
                }
            }
        }

        // The feed goes on past what is loaded: YouTube serves it three shelves at a time, and
        // the site keeps loading as you scroll. Within a screen of the end, ask for the next
        // page; a placeholder shelf stands in while it comes.
        if app.home_has_more() || app.home_more_loading {
            let near_end = ui.cursor().top() < ui.clip_rect().bottom() + ui.clip_rect().height();
            if near_end {
                app.load_home_more();
            }
            if app.home_more_loading {
                ui.add_space(20.0);
                widgets::skeleton_shelf(
                    ui,
                    &app.palette,
                    widgets::SkeletonShelfShape::Cards(widgets::ArtShape::Square),
                    4,
                );
            }
        }
        ui.add_space(theme::PAGE_INSET_X);
    });
}

fn merged_shelf(
    app: &mut App,
    ui: &mut egui::Ui,
    shelf: &Shelf,
    rows: Option<&source_rows::State>,
) {
    ui.add_space(20.0);
    if let Some(rows) = rows {
        if let Some(line) = &shelf.strapline {
            super::shelf_strapline(ui, &app.palette, line);
        }
        ui.label(
            egui::RichText::new(&shelf.title)
                .font(theme::bold(theme::SECTION_PT))
                .color(app.palette.text),
        );
        ui.add_space(4.0);
        source_rows::draw_with_options(app, ui, rows, "home", source_rows::Playback::List, false);
    } else {
        let more = (shelf.home_section() == HomeSection::NewReleases).then_some(Route::NewReleases);
        shelf_carousel(app, ui, "home", shelf, more, crate::ui::MoreButton::ShowAll);
    }
}

/// One Home shelf, in the shape YouTube Music's home gives it: songs as pages of rows, mood
/// chips as chips, and everything else as one row of cards that scrolls sideways, with "Show
/// all" opening the whole shelf. Wrapping every shelf's cards into a grid made each one several
/// rows deep and the page a wall.
fn home_shelf(app: &mut App, ui: &mut egui::Ui, s: &crate::model::Shelf) {
    if s.items.is_empty() {
        return;
    }
    if !s.is_mood_list() && s.items.iter().all(|e| e.track().is_some()) {
        song_shelf(app, ui, "home", s);
        return;
    }
    ui.add_space(20.0);
    // "Show all" only where there is more than the row already holds — a shelf whose header
    // names its own full list. Elsewhere the arrows reach every item, and a page repeating them
    // would add nothing. YouTube Music's own "More" appears on the same shelves.
    let more = (!s.is_mood_list()
        && youtube_shelf(s)
        && s.more_browse_id.is_some()
        && s.more_params.is_some())
    .then(|| Route::HomeShelf(s.title.clone()));
    shelf_carousel(app, ui, "home", s, more, crate::ui::MoreButton::ShowAll);
}

/// A Home shelf's "Show all": every item in it, as a grid. Where the shelf named its own full
/// list (a "more" target), that is fetched and shown; otherwise it is the items Home has.
pub fn shelf_page(app: &mut App, ui: &mut egui::Ui, title: &str) {
    let full = app
        .home_shelf_full
        .as_ref()
        .filter(|(t, _)| t == title)
        .map(|(_, shelf)| std::sync::Arc::clone(shelf));
    let home = app.home.clone();
    let from_home = home.as_ref().and_then(|h| {
        h.iter()
            .find(|s| {
                s.title == title
                    && youtube_shelf(s)
                    && s.more_browse_id.is_some()
                    && s.more_params.is_some()
            })
            .or_else(|| h.iter().find(|s| s.title == title && youtube_shelf(s)))
    });
    crate::ui::page_scroll(app, "home-shelf").show(ui, |ui| {
        crate::ui::page_title(ui, &app.palette, title);
        let Some(shelf) = full.as_deref().or(from_home) else {
            if app.home_loading {
                widgets::skeleton_grid(ui, &app.palette, 2, widgets::ArtShape::Square);
            } else {
                crate::ui::centered(app, ui, "This shelf is no longer on Home.");
            }
            return;
        };
        if let Some(line) = &shelf.strapline {
            ui.label(
                egui::RichText::new(line)
                    .font(theme::medium(12.0))
                    .color(app.palette.dim),
            );
            ui.add_space(8.0);
        }
        let signed_in = app.signed_in();
        let preferred_provider = app.settings.preferred_provider;
        let group_sources = app.can_group_sources();
        let grid = {
            let App {
                palette,
                images,
                backend,
                now,
                library,
                rail_cache,
                album_cache,
                ..
            } = app;
            let ctx = crate::ui::ReleaseCtx {
                playlists: &library.playlists.items,
                signed_in,
                preferred_provider,
                group_sources,
                rail: rail_cache,
                albums: album_cache,
            };
            crate::ui::entry_grid(ui, palette, images, backend, now, &shelf.items, &ctx)
        };
        crate::ui::finish_grid(app, shelf, grid);
        // The full list is still coming: placeholder cards after the ones already here, so the
        // page reads as loading the rest rather than as all there is.
        if full.is_none() && app.home_shelf_loading(title) {
            ui.add_space(8.0);
            widgets::skeleton_grid(ui, &app.palette, 2, widgets::ArtShape::Square);
        }
        ui.add_space(theme::PAGE_INSET_X);
    });
}

/// What a click on the mood bar asked the feed to do.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FilterChoice {
    /// Browse the feed through this chip's params.
    Apply(String),
    /// Back to the default feed.
    Clear,
}

/// The mood bar: the same feed filtered through Workout, Relax and friends.
///
/// Plain buttons like the Moods & genres chips, with the selected one highlighted. Clicking
/// the selected chip clears back to the default feed, as the site does — and while one is
/// selected a ✕ chip closes the row, so clearing is a visible choice rather than a second
/// click on the lit chip. Returns what was clicked, if anything, and the ✕'s response while
/// it is shown — the caller only wants the former; tests use the latter to find the button.
fn filter_row(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    chips: &[crate::model::HomeChip],
) -> (Option<FilterChoice>, Option<egui::Response>) {
    // The clicked index, acted on after the loop — the pattern M4 put everywhere else. Cloning
    // every chip's strings up front was 22 allocations a frame in the render path.
    let mut picked = None;
    let mut clear = None;
    // Wrapped: the moods that do not fit the card's width drop to the next line — the ✕ that
    // appears once one is picked included — rather than scrolling off an edge where the last one
    // was cut mid-word and the rest could not be seen at all.
    ui.horizontal_wrapped(|ui| {
        let mut selected = false;
        for (i, chip) in chips.iter().enumerate() {
            selected |= chip.selected;
            if widgets::chip(ui, palette, &chip.title, chip.selected).clicked() {
                picked = Some(i);
            }
        }
        if selected {
            clear = Some(widgets::icon_chip(ui, palette, Icon::X, 15.0));
        }
    });
    let choice = if clear.as_ref().is_some_and(|r| r.clicked()) {
        Some(FilterChoice::Clear)
    } else if let Some(chip) = picked.and_then(|i| chips.get(i)) {
        if chip.selected {
            Some(FilterChoice::Clear)
        } else {
            Some(FilterChoice::Apply(chip.params.clone()))
        }
    } else {
        None
    };
    (choice, clear)
}

fn filter_chips(app: &mut App, ui: &mut egui::Ui) {
    if app.home_chips.is_empty() || !app.home_sources().youtube {
        return;
    }
    if app.effective_home_source() == HomeSource::Combined {
        let label = if app.home_chips.iter().any(|chip| chip.selected) {
            "YouTube Music only"
        } else {
            "YouTube Music moods"
        };
        ui.label(
            egui::RichText::new(label)
                .font(theme::regular(12.0))
                .color(app.palette.dim),
        );
    }
    let palette = app.palette;
    let (choice, _) = filter_row(ui, &palette, &app.home_chips);
    match choice {
        Some(FilterChoice::Apply(params)) => app.set_home_filter(Some(params)),
        Some(FilterChoice::Clear) => app.set_home_filter(None),
        None => {}
    }
    ui.add_space(4.0);
}

fn sign_in_note(app: &mut App, ui: &mut egui::Ui) {
    if !app.connected_providers().is_empty() || app.has_local_music() || !app.home_sources().youtube
    {
        return;
    }
    ui.label(
        egui::RichText::new("Sign in for Listen again, Mixed for you and Quick picks.")
            .font(theme::regular(12.0))
            .color(app.palette.dim),
    );
    ui.add_space(6.0);
    if widgets::pill_button(ui, &app.palette, "Sign in").clicked() {
        app.sign_in();
    }
    // A failed attempt is reported where it was started, as the account menu does.
    if let Some(error) = &app.sign_in_error {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(error)
                .font(theme::regular(12.0))
                .color(app.palette.danger),
        );
    }
    ui.add_space(4.0);
}

fn youtube_shelf(shelf: &crate::model::Shelf) -> bool {
    !shelf.items.is_empty()
        && shelf
            .items
            .iter()
            .all(|entry| crate::source_groups::entry_provider(entry) == Provider::YouTubeMusic)
}

fn provider_settings(app: &mut App, provider: Provider) {
    app.settings_tab = crate::ui::settings::SettingsTab::Providers;
    app.settings_provider = provider;
    app.navigate(Route::Settings);
}

fn manage_providers(app: &mut App) {
    let provider = app
        .connected_providers()
        .first()
        .copied()
        .or_else(|| app.available_providers().first().copied())
        .unwrap_or(Provider::YouTubeMusic);
    provider_settings(app, provider);
}

fn home_warnings(app: &mut App, ui: &mut egui::Ui) {
    if app.home_warnings.is_empty() {
        return;
    }
    for warning in &app.home_warnings {
        ui.label(
            egui::RichText::new(warning)
                .font(theme::regular(12.0))
                .color(app.palette.danger),
        );
    }
    if widgets::outline_button(ui, &app.palette, "Try again").clicked() {
        app.retry_route();
    }
    ui.add_space(8.0);
}

fn empty_home(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(theme::PAGE_INSET_X);
    match app.effective_home_source() {
        HomeSource::Local if app.has_local_music() => {
            let message = if app.local_scanning {
                "Reading your music folders…"
            } else {
                "Add music folders to fill your Home."
            };
            ui.label(
                egui::RichText::new(message)
                    .font(theme::regular(13.0))
                    .color(app.palette.dim),
            );
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if widgets::pill_button(ui, &app.palette, "Add folders").clicked() {
                    provider_settings(app, Provider::Local);
                }
                if widgets::outline_button(ui, &app.palette, "Manage providers").clicked() {
                    manage_providers(app);
                }
            });
        }
        _ => {
            ui.label(
                egui::RichText::new("Nothing to show on Home yet.")
                    .font(theme::regular(13.0))
                    .color(app.palette.dim),
            );
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if widgets::outline_button(ui, &app.palette, "Try again").clicked() {
                    app.retry_route();
                }
                if widgets::outline_button(ui, &app.palette, "Manage providers").clicked() {
                    manage_providers(app);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::HomeChip;

    fn chip(title: &str, selected: bool) -> HomeChip {
        HomeChip {
            title: title.into(),
            params: format!("params-{title}"),
            selected,
        }
    }

    /// The ✕ is the whole point of the row's clear affordance, so it must be there exactly when
    /// one of the chips is lit — and clicking it must ask for the default feed. Driven for real:
    /// the row is laid out in a headless `Context`, and the ✕'s own rect is clicked.
    #[test]
    fn the_clear_button_appears_with_a_selected_mood_and_clears_it() {
        use egui::{Event, PointerButton, Pos2, vec2};

        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let palette = crate::theme::Palette::dark();

        let mut t = 0.0;
        let clear_rect = std::cell::Cell::new(None);
        let choice = std::cell::RefCell::new(None);
        let mut pass = |chips: &[HomeChip], events: Vec<Event>| {
            t += 1.0 / 60.0;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0))),
                time: Some(t),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                let (picked, clear) = filter_row(ui, &palette, chips);
                *choice.borrow_mut() = picked;
                clear_rect.set(clear.map(|r| r.rect));
            })
            .drop_without_applying_deltas();
        };

        // Nothing selected: no ✕ to click.
        pass(&[chip("Workout", false), chip("Relax", false)], vec![]);
        assert!(
            clear_rect.get().is_none(),
            "the clear button showed while the feed was unfiltered"
        );

        // One selected: it appears, after the chips.
        pass(&[chip("Workout", true), chip("Relax", false)], vec![]);
        let rect = clear_rect
            .get()
            .expect("the clear button did not show with a mood selected");

        // Click it: the row asks for the default feed.
        let centre = rect.center();
        pass(
            &[chip("Workout", true), chip("Relax", false)],
            vec![Event::PointerMoved(centre)],
        );
        pass(
            &[chip("Workout", true), chip("Relax", false)],
            vec![Event::PointerButton {
                pos: centre,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        pass(
            &[chip("Workout", true), chip("Relax", false)],
            vec![Event::PointerButton {
                pos: centre,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(
            choice.borrow().clone(),
            Some(FilterChoice::Clear),
            "clicking the clear button did not ask for the default feed"
        );
    }
}
