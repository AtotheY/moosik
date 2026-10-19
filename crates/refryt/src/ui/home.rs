//! Home: the feed YouTube Music would actually serve.
//!
//! This used to be a moods list and a recent-plays shelf we assembled ourselves, which meant none
//! of YouTube's ranking reached the page. It is now `FEmusic_home` verbatim — Listen again, Mixed
//! for you and Quick picks when signed in — with Explore's shelves appended while the anonymous
//! feed is thin. Its shelves are YouTube's own, but their order is the listener's: the sections
//! named in Settings' "Order of homepage" go where it puts them (see `model::order_home`), and
//! everything else follows in the order YouTube sent.

use crate::app::{App, Route};
use crate::auth::AuthState;
use crate::icons::Icon;
use crate::theme;
use crate::ui::{failed, shelf_carousel, song_shelf};
use crate::widgets;

pub fn page(app: &mut App, ui: &mut egui::Ui) {
    // No "Home" heading: the sidebar already says where you are.
    if app.home_loading && app.home.is_none() {
        // The first row's distance from the card's top, matching the sides.
        ui.add_space(theme::PAGE_INSET_X);
        // The feed's own mood chips head it, as `filter_chips` draws them.
        widgets::skeleton_pills(ui, &app.palette, 10);
        ui.add_space(4.0);
        // The real feed is several titled shelves, mixing grids and row-lists — never one
        // untitled block. Which shelves are coming can't be known before they load, so this is
        // a reasonable approximation of the shape, not a prediction of the exact content.
        // In the order the feed usually opens with: Listen again's cards, Quick picks' song
        // grid, then more cards. Each with the air `home_shelf` and `song_shelf` give theirs.
        ui.add_space(20.0);
        // Signed in, Listen again carries a line over its title — the account's name.
        if app.signed_in() {
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
        sign_in_note(app, ui);
        filter_chips(app, ui);

        // One snapshot for the frame — a refcount bump, not a copy of every shelf. `shelf`
        // needs `&mut App` for navigation and the texture cache, which rules out borrowing
        // the feed out of `app` across the call.
        let Some(shelves) = app.home.clone() else {
            return;
        };
        for s in shelves.iter() {
            home_shelf(app, ui, s);
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
    let more = (!s.is_mood_list() && s.more_browse_id.is_some() && s.more_params.is_some())
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
    let from_home = home
        .as_ref()
        .and_then(|h| h.iter().find(|s| s.title == title));
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
    if app.home_chips.is_empty() {
        return;
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

/// Said once, quietly: the anonymous feed is two shelves, and a session is what unlocks the rest.
fn sign_in_note(app: &mut App, ui: &mut egui::Ui) {
    if matches!(app.auth, AuthState::SignedIn { .. }) {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(
                "You're browsing signed out. Signing in adds your own feed: \
                 Listen again, Mixed for you and Quick picks.",
            )
            .font(theme::regular(12.0))
            .color(app.palette.dim),
        );
    });
    ui.add_space(6.0);
    if widgets::pill_button(ui, &app.palette, "Sign in").clicked() {
        app.sign_in();
    }
    ui.add_space(4.0);
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
