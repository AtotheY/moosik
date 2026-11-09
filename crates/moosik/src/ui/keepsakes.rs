//! Listening keepsakes: the collection shelf, player artwork and artist-pass sheet.
use crate::listening::awards::SongRank;
use crate::listening::collection::{ArtistPass, TIERS};
use crate::{app_icon::AppIcon, icons::Icon, theme, widgets};
use egui::{Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

const PAPER: Color32 = Color32::from_rgb(239, 222, 180);
const GOLD: Color32 = Color32::from_rgb(226, 185, 94);
const INK: Color32 = Color32::from_rgb(60, 45, 33);
const STRIPE: Color32 = Color32::from_rgb(159, 74, 61);

pub(super) fn rank_color(rank: SongRank) -> Color32 {
    super::record_display::metal(rank)
}

/// Personal song rank, always attached to the lower-left of the artwork.
/// Tiny row covers keep the sparkle; player and gallery covers also show rank text.
pub(super) fn rank_badge(ui: &Ui, cover: Rect, completed: u64, ordinal: Option<usize>) {
    let Some(rank) = SongRank::from_completed(completed) else {
        return;
    };
    let size = cover.width().min(cover.height());
    let small = size < 90.;
    let height = if size < 44. {
        14.
    } else if small {
        18.
    } else {
        24.
    };
    let font = theme::semibold(if small { 9. } else { 11. });
    let label = if size < 44. {
        String::new()
    } else if size >= 130. {
        ordinal.map_or_else(
            || rank.name().to_owned(),
            |place| format!("#{place} · {}", rank.name()),
        )
    } else {
        ordinal.map_or_else(|| rank.name().to_owned(), |place| format!("#{place}"))
    };
    let pad = if small { 4. } else { 7. };
    let star_r = if small { 4. } else { 5.5 };
    let star_slot = star_r * 2. + 1.;
    let gap = if label.is_empty() { 0. } else { 3. };
    let text_width = widgets::text_width(ui, &label, &font);
    let width = (pad * 2. + star_slot + gap + text_width).min(size - 6.);
    let badge = Rect::from_min_size(
        pos2(cover.left() + 3., cover.bottom() - height - 3.),
        vec2(width, height),
    );
    let painter = ui.painter().with_clip_rect(cover.intersect(ui.clip_rect()));
    let color = rank_color(rank);
    painter.rect_filled(
        badge,
        height * 0.38,
        Color32::from_rgba_unmultiplied(10, 14, 20, 226),
    );
    painter.rect_stroke(
        badge,
        height * 0.38,
        Stroke::new(0.9, color.gamma_multiply(0.78)),
        egui::StrokeKind::Inside,
    );
    // Draw the glint with the clipped painter so it remains inside rounded cover art.
    let star_at = pos2(badge.left() + pad + star_r, badge.center().y);
    let star_ui = ui.painter().with_clip_rect(badge.intersect(ui.clip_rect()));
    let narrow = star_r * 0.22;
    star_ui.circle_filled(star_at, star_r * 1.55, color.gamma_multiply(0.15));
    for points in [
        vec![
            pos2(star_at.x, star_at.y - star_r),
            pos2(star_at.x + narrow, star_at.y),
            pos2(star_at.x, star_at.y + star_r),
            pos2(star_at.x - narrow, star_at.y),
        ],
        vec![
            pos2(star_at.x - star_r, star_at.y),
            pos2(star_at.x, star_at.y - narrow),
            pos2(star_at.x + star_r, star_at.y),
            pos2(star_at.x, star_at.y + narrow),
        ],
    ] {
        star_ui.add(Shape::convex_polygon(points, color, Stroke::NONE));
    }
    star_ui.circle_filled(star_at, (star_r * 0.17).max(0.6), Color32::WHITE);
    if !label.is_empty() {
        let remaining = (badge.right() - (star_at.x + star_r + gap) - pad).max(0.);
        let text = widgets::truncate_to(ui, &label, font.clone(), remaining);
        painter.text(
            pos2(star_at.x + star_r + gap, badge.center().y),
            Align2::LEFT_CENTER,
            text,
            font,
            color,
        );
    }
}

fn star(ui: &Ui, center: Pos2, radius: f32, ink: Color32) {
    for i in 0..5 {
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / 5.0;
        let half = std::f32::consts::PI / 5.0;
        ui.painter().add(Shape::convex_polygon(
            vec![
                center,
                center + egui::Vec2::angled(angle - half) * radius * 0.44,
                center + egui::Vec2::angled(angle) * radius,
                center + egui::Vec2::angled(angle + half) * radius * 0.44,
            ],
            ink,
            Stroke::NONE,
        ));
    }
}

fn ticket(ui: &Ui, r: Rect, tier: usize, background: Color32) {
    let tiny = r.height() < 20.;
    let painter = ui.painter();
    painter.rect_filled(
        r,
        if tiny { 2 } else { 6 },
        if tier == 4 { GOLD } else { PAPER },
    );
    let stub = r.right() - if tiny { 4. } else { 30. };
    if tier >= 2 {
        painter.rect_filled(
            Rect::from_min_max(
                pos2(r.left() + 2., r.top() + 1.),
                pos2(r.left() + if tiny { 4. } else { 6. }, r.bottom() - 1.),
            ),
            1,
            STRIPE,
        );
    }
    let radius = if tiny { 1.55 } else { 4. };
    for x in [r.left(), r.right()] {
        painter.circle_filled(pos2(x, r.center().y), radius, background);
    }
    let step = if tiny { 3.0 } else { 7.0 };
    let mut y = r.top() + 2.;
    while y < r.bottom() - 1. {
        painter.vline(
            stub,
            y..=(y + step * 0.42).min(r.bottom() - 1.),
            Stroke::new(0.8, INK.gamma_multiply(0.50)),
        );
        y += step;
    }
    if tier >= 3 {
        let x = if tiny { r.center().x } else { r.right() - 15. };
        star(ui, pos2(x, r.center().y), if tiny { 3.5 } else { 7. }, INK);
    }
}

fn pass_card(
    ui: &mut Ui,
    pass: &ArtistPass,
    width: f32,
    back: bool,
    background: Color32,
) -> egui::Response {
    let tier = pass.stamps.len();
    let height = if back { 72. + 40. * tier as f32 } else { 102. };
    let (r, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    ticket(ui, r, tier, background);
    let left = r.left() + 13.;
    let write = |y, text: &str, font: egui::FontId| {
        let text = widgets::truncate_to(ui, text, font.clone(), width - 54.);
        ui.painter().text(
            pos2(left, r.top() + y),
            Align2::LEFT_CENTER,
            text,
            font,
            INK,
        );
    };
    if back {
        write(15., "ARTIST PASS · STAMP HISTORY", theme::semibold(10.));
        write(35., &pass.name, theme::semibold(14.));
        ui.painter().hline(
            (left + 1.)..=(r.right() - 42.),
            r.top() + 47.,
            Stroke::new(0.8, INK.gamma_multiply(0.35)),
        );
        for (i, stamp) in pass.stamps.iter().enumerate() {
            let y = 59. + 40. * i as f32;
            write(
                y,
                &format!("{} · {}", TIERS[i], stamp.day),
                theme::medium(11.),
            );
            write(
                y + 16.,
                &format!("Played when earned: {}", stamp.song),
                theme::regular(11.),
            );
        }
        let next = pass.next_days().map_or_else(
            || "All Access earned".to_owned(),
            |days| format!("Next: {} · {days} listening days", TIERS[tier]),
        );
        write(58. + 40. * tier as f32, &next, theme::medium(11.));
    } else {
        write(15., "ARTIST PASS", theme::semibold(10.));
        write(37., TIERS[tier - 1], theme::semibold(12.));
        write(62., &pass.name, theme::bold(19.));
        let progress = pass.next_days().map_or_else(
            || format!("{} listening days", pass.days()),
            |next| format!("{}/{} listening days to {}", pass.days(), next, TIERS[tier]),
        );
        write(86., &progress, theme::regular(11.));
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!(
                "{} artist pass, {} stamp history",
                pass.name,
                TIERS[tier - 1]
            ),
        )
    });
    response
}

pub fn player_pass(
    ui: &mut Ui,
    palette: &theme::Palette,
    origin: Pos2,
    pass: &ArtistPass,
    key: &str,
) {
    let r = Rect::from_center_size(origin + vec2(9., 0.), vec2(18., 18.));
    let hit = ui.interact(r, ui.id().with(("artist-pass", key)), Sense::click());
    ticket(
        ui,
        Rect::from_center_size(r.center(), vec2(15., 9.)),
        pass.stamps.len(),
        palette.window,
    );
    hit.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("Open {}’s artist pass and stamp history", pass.name),
        )
    });
    let popup = egui::Popup::menu(&hit)
        .align(egui::RectAlign::TOP_START)
        .gap(5.)
        .frame(egui::Frame::NONE)
        .width(282.);
    if !popup.is_open() {
        hit.clone().on_hover_ui(|ui| {
            pass_card(ui, pass, 282., false, ui.visuals().extreme_bg_color);
            ui.label(
                egui::RichText::new("Click to see stamp history")
                    .font(theme::regular(11.))
                    .color(palette.secondary),
            );
        });
    }
    popup.show(|ui| {
        pass_card(ui, pass, 282., true, palette.panel);
    });
}

#[derive(Default, PartialEq, Clone, Copy)]
enum Category {
    Icons,
    #[default]
    Artists,
    Songs,
}
#[derive(Default)]
pub(super) struct Shelf {
    browse: Option<Category>,
    in_progress: bool,
    search: String,
    revision: Option<u64>,
    artists: Vec<String>,
    songs: Vec<String>,
}
fn by_time<'a>(items: impl Iterator<Item = (&'a String, f64)>) -> Vec<String> {
    let mut items: Vec<_> = items.collect();
    items.sort_by(|(a, ta), (b, tb)| tb.total_cmp(ta).then(a.cmp(b)));
    items.into_iter().map(|(key, _)| key.clone()).collect()
}
fn reward_row(
    ui: &mut Ui,
    p: &theme::Palette,
    title: &str,
    detail: &str,
    mark: impl FnOnce(&Ui, Rect),
) -> egui::Response {
    let (r, hit) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::ROW_HEIGHT),
        Sense::hover(),
    );
    if ui.is_rect_visible(r) {
        let tile = Rect::from_center_size(pos2(r.left() + 22., r.center().y), vec2(38., 38.));
        mark(ui, tile);
        let x = tile.right() + 12.;
        for (label, dy, font, color) in [
            (title, -9., theme::semibold(14.), p.text),
            (detail, 10., theme::regular(12.), p.secondary),
        ] {
            ui.painter().text(
                pos2(x, r.center().y + dy),
                Align2::LEFT_CENTER,
                widgets::truncate_to(ui, label, font.clone(), r.right() - x),
                font,
                color,
            );
        }
    }
    hit.on_hover_text(format!("{title}\n{detail}"))
}
fn artist_row(ui: &mut Ui, p: &theme::Palette, pass: &ArtistPass) {
    let tier = pass.stamps.len();
    let status = match tier {
        0 => format!(
            "Next: Admit One · {}/10 songs or {}/5 listening days",
            pass.songs(),
            pass.days()
        ),
        4 => format!("All Access · {} listening days", pass.days()),
        _ => format!(
            "{} earned · {}/{} listening days to {}",
            TIERS[tier - 1],
            pass.days(),
            pass.next_days().unwrap(),
            TIERS[tier]
        ),
    };
    if tier > 0 {
        // Preserve the ticket's dated stamp history on both the shelf and the player.
        let hit = reward_row(ui, p, &pass.name, &status, |ui, t| {
            ticket(ui, t.shrink2(vec2(0., 8.)), tier, p.panel)
        });
        let click = ui
            .interact(hit.rect, hit.id.with("stamps"), Sense::click())
            .on_hover_text(format!("See {}’s artist pass and stamps", pass.name));
        egui::Popup::menu(&click)
            .frame(egui::Frame::NONE)
            .show(|ui| {
                pass_card(ui, pass, 282., true, p.panel);
            });
    } else {
        reward_row(ui, p, &pass.name, &status, |ui, t| {
            ui.painter().rect_stroke(
                t.shrink2(vec2(0., 8.)),
                3.,
                Stroke::new(1., p.dim),
                egui::StrokeKind::Inside,
            );
        });
    }
}
fn card_width(ui: &Ui) -> (usize, f32) {
    let available = ui.available_width();
    let cols = (((available + 10.) / 170.).floor() as usize).clamp(1, 6);
    let width = ((available - 10. * (cols - 1) as f32) / cols as f32).min(192.);
    (cols, width)
}

fn card_shell(
    ui: &mut Ui,
    p: &theme::Palette,
    width: f32,
    height: f32,
    sense: Sense,
) -> (Rect, egui::Response) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
    let fill = widgets::mix(p.surface, p.surface_hover, widgets::lit(ui, &response));
    ui.painter().rect_filled(rect, 12., fill);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            12.,
            Stroke::new(1., p.accent),
            egui::StrokeKind::Inside,
        );
    }
    (rect, response)
}

fn card_line(ui: &Ui, rect: Rect, y: f32, text: &str, font: egui::FontId, color: Color32) {
    ui.painter().text(
        pos2(rect.left() + 12., rect.top() + y),
        Align2::LEFT_CENTER,
        widgets::truncate_to(ui, text, font.clone(), (rect.width() - 24.).max(10.)),
        font,
        color,
    );
}

fn next_finish(ui: &mut Ui, p: &theme::Palette, peak: Option<u32>) {
    let finishes: Vec<_> = AppIcon::ALL
        .into_iter()
        .filter_map(|icon| Some((icon, icon.milestone()?)))
        .collect();
    let minutes = peak.unwrap_or(0);
    let next = finishes.iter().find(|(_, hours)| minutes < hours * 60);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 94.), Sense::hover());
    ui.painter().rect_filled(r, 12., p.surface);
    let icon = next.map_or(AppIcon::Carbon, |(icon, _)| *icon);
    let art = Rect::from_min_size(pos2(r.left() + 14., r.top() + 14.), vec2(66., 66.));
    if let Some(tex) = icon.texture(ui.ctx()) {
        ui.painter().image(
            tex.id(),
            art,
            Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)),
            Color32::WHITE,
        );
    }
    let left = r.left() + 96.;
    let right = r.right() - 18.;
    let width = (right - left).max(10.);
    let title = next.map_or("Every app finish unlocked".to_owned(), |(icon, _)| {
        format!("Next finish: {}", icon.label())
    });
    ui.painter().text(
        pos2(left, r.top() + 28.),
        Align2::LEFT_CENTER,
        widgets::truncate_to(ui, &title, theme::semibold(15.), width),
        theme::semibold(15.),
        p.text,
    );
    let detail = match (peak, next) {
        (None, _) => "Loading your best listening week…".to_owned(),
        (Some(best), Some((_, hours))) => {
            format!("Best week  {}h {:02}m / {hours}h", best / 60, best % 60)
        }
        (Some(best), None) => format!("Best listening week  {}h {:02}m", best / 60, best % 60),
    };
    ui.painter().text(
        pos2(left, r.top() + 52.),
        Align2::LEFT_CENTER,
        widgets::truncate_to(ui, &detail, theme::regular(12.), width),
        theme::regular(12.),
        p.secondary,
    );
    let rail = Rect::from_min_size(pos2(left, r.bottom() - 16.), vec2(width, 4.));
    ui.painter().rect_filled(rail, 2., p.surface_active);
    let fraction = next
        .map_or(1., |(_, hours)| minutes as f32 / (*hours * 60) as f32)
        .clamp(0., 1.);
    if fraction > 0. {
        ui.painter().rect_filled(
            Rect::from_min_size(rail.min, vec2(rail.width() * fraction, 4.)),
            2.,
            p.accent,
        );
    }
}

fn section_header(ui: &mut Ui, p: &theme::Palette, title: &str, count: &str, open: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .font(theme::semibold(theme::SECTION_PT))
                .color(p.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            clicked =
                widgets::outline_button(ui, p, if open { "Close" } else { "Browse all" }).clicked();
        });
    });
    ui.label(
        egui::RichText::new(count)
            .font(theme::regular(12.))
            .color(p.secondary),
    );
    ui.add_space(10.);
    clicked
}

fn icon_card(ui: &mut Ui, p: &theme::Palette, icon: AppIcon, hours: u32, peak: u32, width: f32) {
    let (r, response) = card_shell(ui, p, width, 142., Sense::hover());
    let art = Rect::from_center_size(pos2(r.center().x, r.top() + 52.), vec2(70., 70.));
    if let Some(tex) = icon.texture(ui.ctx()) {
        ui.painter().image(
            tex.id(),
            art,
            Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)),
            if peak >= hours * 60 {
                Color32::WHITE
            } else {
                Color32::from_white_alpha(170)
            },
        );
    }
    card_line(ui, r, 106., icon.label(), theme::semibold(13.), p.text);
    card_line(
        ui,
        r,
        125.,
        if peak >= hours * 60 {
            "Earned".to_owned()
        } else {
            format!("Unlock at {hours}h")
        }
        .as_str(),
        theme::regular(11.),
        if peak >= hours * 60 {
            p.accent_text()
        } else {
            p.secondary
        },
    );
    response.on_hover_text(format!("{} · {}h listening week", icon.label(), hours));
}

fn artist_card(ui: &mut Ui, p: &theme::Palette, pass: &ArtistPass, width: f32) {
    let tier = pass.stamps.len();
    let (r, response) = card_shell(
        ui,
        p,
        width,
        176.,
        if tier > 0 {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let t = Rect::from_min_size(
        pos2(r.left() + 12., r.top() + 14.),
        vec2(r.width() - 24., 72.),
    );
    if tier > 0 {
        ticket(ui, t, tier, p.surface);
        ui.painter().text(
            pos2(t.left() + 12., t.top() + 14.),
            Align2::LEFT_CENTER,
            "ARTIST PASS",
            theme::semibold(10.),
            INK,
        );
        ui.painter().text(
            pos2(t.left() + 12., t.top() + 33.),
            Align2::LEFT_CENTER,
            TIERS[tier - 1],
            theme::semibold(10.),
            INK,
        );
        ui.painter().text(
            pos2(t.left() + 12., t.top() + 55.),
            Align2::LEFT_CENTER,
            widgets::truncate_to(ui, &pass.name, theme::bold(16.), t.width() - 44.),
            theme::bold(16.),
            INK,
        );
    } else {
        ui.painter()
            .rect_stroke(t, 6., Stroke::new(1., p.dim), egui::StrokeKind::Inside);
        Icon::Music
            .image(p.dim, 32.)
            .paint_at(ui, Rect::from_center_size(t.center(), vec2(32., 32.)));
    }
    card_line(ui, r, 108., &pass.name, theme::semibold(13.), p.text);
    let (status, progress_label) = match pass.next_days() {
        Some(next) => (
            format!("Next: {}", TIERS[tier]),
            format!("{}/{} listening days", pass.days(), next),
        ),
        None if tier == 0 => (
            "Next: Admit One".to_owned(),
            format!("{}/10 songs · {}/5 days", pass.songs(), pass.days()),
        ),
        None => (
            "All Access earned".to_owned(),
            format!("{} listening days", pass.days()),
        ),
    };
    card_line(
        ui,
        r,
        128.,
        &status,
        theme::regular(11.),
        if tier > 0 {
            p.accent_text()
        } else {
            p.secondary
        },
    );
    card_line(
        ui,
        r,
        147.,
        &progress_label,
        theme::regular(11.),
        p.secondary,
    );
    let progress = if tier == 0 {
        (pass.songs() as f32 / 10.).max(pass.days() as f32 / 5.)
    } else {
        pass.next_days()
            .map_or(1., |next| pass.days() as f32 / next as f32)
    }
    .clamp(0., 1.);
    let bar = Rect::from_min_size(
        pos2(r.left() + 12., r.bottom() - 12.),
        vec2(r.width() - 24., 3.),
    );
    ui.painter().rect_filled(bar, 2., p.surface_active);
    if progress > 0. {
        ui.painter().rect_filled(
            Rect::from_min_size(bar.min, vec2(bar.width() * progress, 3.)),
            2.,
            p.accent,
        );
    }
    let response = response.on_hover_text(if tier > 0 {
        format!("See {}’s artist pass and dated stamps", pass.name)
    } else {
        format!(
            "Hear 10 different {} songs or listen on 5 days to earn Admit One",
            pass.name
        )
    });
    if tier > 0 {
        egui::Popup::menu(&response)
            .frame(egui::Frame::NONE)
            .show(|ui| {
                pass_card(ui, pass, 282., true, p.panel);
            });
    }
}

fn empty_preview(ui: &mut Ui, p: &theme::Palette, icon: Icon, title: &str, detail: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 98.), Sense::hover());
    ui.painter().rect_filled(r, 12., p.surface);
    icon.image(p.dim, 32.).paint_at(
        ui,
        Rect::from_center_size(pos2(r.left() + 34., r.center().y), vec2(32., 32.)),
    );
    let left = r.left() + 64.;
    ui.painter().text(
        pos2(left, r.top() + 36.),
        Align2::LEFT_CENTER,
        title,
        theme::semibold(14.),
        p.text,
    );
    ui.painter().text(
        pos2(left, r.top() + 59.),
        Align2::LEFT_CENTER,
        widgets::truncate_to(
            ui,
            detail,
            theme::regular(12.),
            (r.right() - left - 14.).max(10.),
        ),
        theme::regular(12.),
        p.secondary,
    );
}

impl Shelf {
    pub(super) fn sheet(
        &mut self,
        ui: &mut Ui,
        p: &theme::Palette,
        ledger: &crate::listening::Ledger,
        mut paint_cover: impl FnMut(&Ui, Rect, Option<&std::sync::Arc<str>>),
    ) {
        let Some(c) = ledger.collection.as_ref() else {
            let error = ledger.error.as_deref().or(ledger
                .collection_unavailable
                .then_some("Collection unavailable for this listening history."));
            ui.label(error.unwrap_or("Loading collection…"));
            return;
        };
        if self.revision != Some(c.revision) {
            self.artists = by_time(c.passes.iter().map(|(k, v)| (k, v.listen_ms)));
            self.songs = crate::listening::awards::ranked_song_ids(c)
                .into_iter()
                .map(str::to_owned)
                .collect();
            self.revision = Some(c.revision);
        }
        let peak = ledger.peak_week_minutes;
        let minutes = peak.unwrap_or(0);
        let finish_icons: Vec<_> = AppIcon::ALL
            .into_iter()
            .filter_map(|icon| Some((icon, icon.milestone()?)))
            .collect();
        let finish_count = finish_icons
            .iter()
            .filter(|(_, h)| minutes >= h * 60)
            .count();
        let pass_count = c
            .passes
            .values()
            .filter(|pass| !pass.stamps.is_empty())
            .count();
        self.song_section(ui, p, c, &mut paint_cover);
        ui.add_space(26.);
        next_finish(ui, p, peak);
        ui.add_space(26.);

        let icon_open = self.browse == Some(Category::Icons);
        if section_header(
            ui,
            p,
            "App finishes",
            &format!(
                "{finish_count} of {} earned · your best listening week unlocks more",
                finish_icons.len()
            ),
            icon_open,
        ) {
            self.browse = if icon_open {
                None
            } else {
                Some(Category::Icons)
            };
            self.search.clear();
        }
        let (columns, width) = card_width(ui);
        let limit = if self.browse == Some(Category::Icons) {
            finish_icons.len()
        } else {
            columns
        };
        let first = if self.browse == Some(Category::Icons) {
            0
        } else {
            finish_count
                .saturating_sub(limit.saturating_sub(1))
                .min(finish_icons.len().saturating_sub(limit))
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10., 10.);
            for &(icon, hours) in finish_icons.iter().skip(first).take(limit) {
                icon_card(ui, p, icon, hours, minutes, width);
            }
        });
        ui.add_space(26.);

        let artist_open = self.browse == Some(Category::Artists);
        if section_header(
            ui,
            p,
            "Artist passes",
            &format!(
                "{pass_count} earned · {} artists in progress",
                c.passes.len() - pass_count
            ),
            artist_open,
        ) {
            self.browse = if artist_open {
                None
            } else {
                Some(Category::Artists)
            };
            self.in_progress = pass_count == 0;
            self.search.clear();
        }
        ui.label(
            egui::RichText::new(
                "Admit One: 10 different songs or 5 listening days · more stamps at 15, 30 and 60 days",
            )
            .font(theme::regular(11.))
            .color(p.secondary),
        );
        ui.add_space(8.);
        if self.browse == Some(Category::Artists) {
            self.browser_controls(ui, p, "Find an artist");
            let query = self.search.trim().to_lowercase();
            let keys: Vec<_> = self
                .artists
                .iter()
                .filter(|key| {
                    let pass = &c.passes[*key];
                    pass.stamps.is_empty() == self.in_progress
                        && (query.is_empty() || pass.name.to_lowercase().contains(&query))
                })
                .collect();
            if keys.is_empty() {
                ui.label(egui::RichText::new("No matching artist passes yet").color(p.secondary));
            }
            super::virtual_rows(ui, theme::ROW_HEIGHT, keys.len(), |ui, i| {
                let key = keys[i];
                ui.push_id(key, |ui| artist_row(ui, p, &c.passes[key]));
            });
        } else {
            let has_progress = self
                .artists
                .iter()
                .any(|key| c.passes[key].stamps.is_empty());
            let earned_slots = columns - usize::from(has_progress && columns > 1);
            let mut keys: Vec<_> = self
                .artists
                .iter()
                .filter(|key| !c.passes[*key].stamps.is_empty())
                .take(earned_slots)
                .collect();
            keys.extend(
                self.artists
                    .iter()
                    .filter(|key| c.passes[*key].stamps.is_empty())
                    .take(columns.saturating_sub(keys.len())),
            );
            if keys.is_empty() {
                empty_preview(
                    ui,
                    p,
                    Icon::Music,
                    "Your first artist pass",
                    "Hear 10 different songs by one artist or listen on 5 days.",
                );
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(10., 10.);
                    for key in keys {
                        ui.push_id(key, |ui| artist_card(ui, p, &c.passes[key], width));
                    }
                });
            }
        }
        ui.add_space(theme::PAGE_INSET_X);
    }

    fn song_section(
        &mut self,
        ui: &mut Ui,
        p: &theme::Palette,
        collection: &crate::listening::collection::Collection,
        paint_cover: &mut impl FnMut(&Ui, Rect, Option<&std::sync::Arc<str>>),
    ) {
        let earned = collection
            .sleeves
            .values()
            .filter(|song| SongRank::from_completed(song.completed).is_some())
            .count();
        let open = self.browse == Some(Category::Songs);
        if section_header(
            ui,
            p,
            "Your records",
            &format!(
                "{earned} ranked songs · {} working toward Iron",
                collection.sleeves.len() - earned
            ),
            open,
        ) {
            self.browse = if open { None } else { Some(Category::Songs) };
            self.in_progress = earned == 0;
            self.search.clear();
        }
        ui.label(
            egui::RichText::new(
                "Ranked by full listens · listening time also includes partial plays",
            )
            .font(theme::regular(11.))
            .color(p.secondary),
        );
        ui.add_space(9.);
        super::collection_gallery::ladder(ui, p);
        ui.add_space(12.);
        let top: Vec<_> = self
            .songs
            .iter()
            .take(3)
            .enumerate()
            .map(|(i, key)| (&collection.sleeves[key], i + 1))
            .collect();
        if top.is_empty() {
            empty_preview(
                ui,
                p,
                Icon::Disc3,
                "Your first Iron record",
                "Finish one song 10 times to place it on your shelf.",
            );
        } else {
            super::collection_gallery::featured(ui, p, &top, paint_cover);
        }
        if self.songs.len() <= 3 && self.browse != Some(Category::Songs) {
            return;
        }
        ui.add_space(18.);
        ui.label(
            egui::RichText::new("More on your shelf")
                .font(theme::semibold(15.))
                .color(p.text),
        );
        ui.add_space(7.);
        if self.browse == Some(Category::Songs) {
            self.browser_controls(ui, p, "Find a song or artist");
            let query = self.search.trim().to_lowercase();
            let keys: Vec<_> = self
                .songs
                .iter()
                .enumerate()
                .filter(|(_, key)| {
                    let song = &collection.sleeves[*key];
                    SongRank::from_completed(song.completed).is_none() == self.in_progress
                        && (query.is_empty()
                            || song.title.to_lowercase().contains(&query)
                            || song.artist.to_lowercase().contains(&query))
                })
                .collect();
            if keys.is_empty() {
                ui.label(egui::RichText::new("No matching songs yet").color(p.secondary));
            } else {
                let row_height = if ui.available_width() < 520. {
                    67.
                } else {
                    theme::ROW_HEIGHT
                };
                super::virtual_rows(ui, row_height, keys.len(), |ui, i| {
                    let (position, key) = keys[i];
                    ui.push_id(key, |ui| {
                        super::collection_gallery::compact_row(
                            ui,
                            p,
                            &collection.sleeves[key],
                            position + 1,
                            paint_cover,
                        )
                    });
                });
            }
        } else {
            for (i, key) in self.songs.iter().enumerate().skip(3).take(3) {
                ui.push_id(key, |ui| {
                    super::collection_gallery::compact_row(
                        ui,
                        p,
                        &collection.sleeves[key],
                        i + 1,
                        paint_cover,
                    )
                });
                ui.add_space(6.);
            }
        }
    }

    fn browser_controls(&mut self, ui: &mut Ui, p: &theme::Palette, placeholder: &str) {
        ui.horizontal_wrapped(|ui| {
            widgets::text_input(
                ui,
                p,
                &mut self.search,
                placeholder,
                ui.available_width().min(300.),
            );
            if widgets::chip(ui, p, "Earned", !self.in_progress).clicked() {
                self.in_progress = false;
            }
            if widgets::chip(ui, p, "In progress", self.in_progress).clicked() {
                self.in_progress = true;
            }
        });
        ui.add_space(8.);
    }
}
