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
    match rank {
        SongRank::Iron => Color32::from_rgb(135, 151, 164),
        SongRank::Bronze => Color32::from_rgb(210, 143, 95),
        SongRank::Silver => Color32::from_rgb(202, 215, 225),
        SongRank::Gold => Color32::from_rgb(239, 195, 105),
        SongRank::Platinum => Color32::from_rgb(222, 235, 242),
        SongRank::DoublePlatinum => Color32::from_rgb(217, 187, 255),
        SongRank::Diamond => Color32::from_rgb(154, 227, 245),
    }
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

fn pass_details(ui: &Ui, hit: egui::Response, p: &theme::Palette, pass: &ArtistPass) {
    let tier = pass.stamps.len();
    let mut tip = egui::Tooltip::for_enabled(&hit);
    tip.popup = tip.popup.frame(egui::Frame::NONE);
    tip.show(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(306., 136.), Sense::hover());
        ticket(ui, r, tier, p.overlay);
        let left = r.left() + 16.;
        let line = |y, value: &str, font: egui::FontId| {
            ui.painter().text(
                pos2(left, r.top() + y),
                Align2::LEFT_CENTER,
                widgets::truncate_to(ui, value, font.clone(), r.width() - 60.),
                font,
                INK,
            );
        };
        let level = tier.checked_sub(1).map_or("In progress", |i| TIERS[i]);
        line(
            17.,
            &format!("ARTIST PASS   /   {}", level.to_uppercase()),
            theme::semibold(10.),
        );
        line(42., &pass.name, theme::bold(21.));
        for (offset, value, label) in [
            (0., pass.songs().to_string(), "Songs heard"),
            (85., pass.days().to_string(), "Listening days"),
            (
                182.,
                super::collection_gallery::elapsed(pass.listen_ms),
                "Time listened",
            ),
        ] {
            ui.painter().text(
                pos2(left + offset, r.top() + 73.),
                Align2::LEFT_CENTER,
                value,
                theme::bold(16.),
                INK,
            );
            ui.painter().text(
                pos2(left + offset, r.top() + 91.),
                Align2::LEFT_CENTER,
                label,
                theme::regular(10.),
                INK,
            );
        }
        let next = match pass.next_days() {
            Some(goal) => format!(
                "Next stamp  ·  {} in {} listening days",
                TIERS[tier],
                goal - pass.days()
            ),
            None if tier == 0 => "Admit One at 10 songs or 5 listening days".into(),
            None => "All Access earned".into(),
        };
        ui.painter().hline(
            left..=r.right() - 42.,
            r.top() + 106.,
            Stroke::new(0.6, INK.gamma_multiply(0.35)),
        );
        line(120., &next, theme::medium(11.));
    });
    if !pass.stamps.is_empty() {
        egui::Popup::menu(&hit)
            .frame(widgets::menu_frame(ui, p))
            .show(|ui| {
                for (tier, stamp) in TIERS.iter().zip(&pass.stamps) {
                    ui.label(
                        egui::RichText::new(format!("{tier} · {}", stamp.day))
                            .font(theme::semibold(12.)),
                    );
                    ui.label(egui::RichText::new(&stamp.song).color(p.secondary));
                }
            });
    }
}

pub fn player_pass(ui: &mut Ui, p: &theme::Palette, origin: Pos2, pass: &ArtistPass, key: &str) {
    let rect = Rect::from_center_size(origin + vec2(9., 0.), vec2(18., 18.));
    let hit = ui.interact(rect, ui.id().with(("artist-pass", key)), Sense::click());
    ticket(
        ui,
        Rect::from_center_size(rect.center(), vec2(15., 9.)),
        pass.stamps.len(),
        p.window,
    );
    pass_details(ui, hit, p, pass);
}

#[derive(Default, PartialEq, Clone, Copy)]
enum Category {
    #[default]
    Records,
    Artists,
    Finishes,
}
#[derive(Default, PartialEq, Clone, Copy)]
enum RecordView {
    #[default]
    Case,
    Crate(SongRank),
    Find,
}
#[derive(Default)]
pub(super) struct Shelf {
    category: Category,
    selected: Option<String>,
    search: String,
    status: usize,
    revision: Option<u64>,
    artists: Vec<String>,
    records: super::collection_gallery::RecordCase,
    record_view: RecordView,
}

fn columns(ui: &mut Ui, p: &theme::Palette, left: &str, right: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(left)
                .font(theme::semibold(12.))
                .color(p.secondary),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(right)
                    .font(theme::regular(12.))
                    .color(p.secondary),
            );
        });
    });
}

fn finishes(ui: &mut Ui, p: &theme::Palette, peak: Option<u32>, current: Option<u32>) {
    let icons: Vec<_> = AppIcon::ALL
        .into_iter()
        .filter_map(|i| Some((i, i.milestone()?)))
        .collect();
    let width = ui.available_width();
    // Balance the rows when the full set cannot fit across the window.
    let capacity = (((width + 8.) / 86.).floor() as usize).clamp(1, icons.len());
    let rows = icons.len().div_ceil(capacity);
    let columns_count = icons.len().div_ceil(rows);
    let cell = ((width - (columns_count - 1) as f32 * 8.) / columns_count as f32).min(132.);
    let earned = icons
        .iter()
        .filter(|(_, h)| peak.is_some_and(|m| m >= h * 60))
        .count();
    if let (Some(best), Some(now)) = (peak, current) {
        let time = |m: u32| super::collection_gallery::elapsed(f64::from(m) * 60_000.);
        if let Some((icon, hours)) = icons.get(earned) {
            let goal = hours * 60;
            columns(
                ui,
                p,
                &format!("This week  ·  {}", time(now)),
                &format!("{} at {hours}h", icon.label()),
            );
            let (rail, _) = ui.allocate_exact_size(vec2(width, 5.), Sense::hover());
            ui.painter().rect_filled(rail, 3., p.surface_active);
            let fraction = (now as f32 / goal as f32).min(1.);
            if fraction > 0. {
                ui.painter().rect_filled(
                    Rect::from_min_size(rail.min, vec2(rail.width() * fraction, 5.)),
                    3.,
                    p.accent,
                );
            }
            columns(
                ui,
                p,
                &format!("{} to {}", time(goal.saturating_sub(now)), icon.label()),
                &format!("Best week  {}", time(best)),
            );
        } else {
            columns(
                ui,
                p,
                &format!("This week  ·  {}", time(now)),
                &format!("All finishes earned  ·  Best week {}", time(best)),
            );
        }
    }
    ui.add_space(20.);
    for row in icons.chunks(columns_count) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            for &(icon, hours) in row {
                let (r, hit) = ui.allocate_exact_size(vec2(cell, 108.), Sense::hover());
                let art = Rect::from_center_size(pos2(r.center().x, r.top() + 32.), vec2(60., 60.));
                let unlocked = peak.is_some_and(|m| m >= hours * 60);
                if let Some(tex) = icon.texture(ui.ctx()) {
                    ui.painter().image(
                        tex.id(),
                        art,
                        Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)),
                        if unlocked {
                            Color32::WHITE
                        } else {
                            Color32::from_white_alpha(170)
                        },
                    );
                }
                ui.painter().text(
                    pos2(r.center().x, r.top() + 77.),
                    Align2::CENTER_CENTER,
                    icon.label(),
                    theme::semibold(13.),
                    p.text,
                );
                let status = if unlocked {
                    "Earned".into()
                } else {
                    format!("{hours}h week")
                };
                ui.painter().text(
                    pos2(r.center().x, r.top() + 94.),
                    Align2::CENTER_CENTER,
                    status,
                    theme::regular(11.),
                    if unlocked {
                        p.accent_text()
                    } else {
                        p.secondary
                    },
                );
                hit.on_hover_text(format!("{} · {hours}h listening week", icon.label()));
            }
        });
        ui.add_space(10.);
    }
}

impl Shelf {
    fn filters(&mut self, ui: &mut Ui, p: &theme::Palette, hint: &str) {
        const OPTIONS: [&str; 3] = ["All", "Earned", "In progress"];
        ui.horizontal_wrapped(|ui| {
            widgets::text_input(
                ui,
                p,
                &mut self.search,
                hint,
                (ui.available_width() - 125.).clamp(100., 260.),
            );
            if let Some(picked) = widgets::text_dropdown(
                ui,
                p,
                ui.id().with("collection-status"),
                OPTIONS[self.status],
                &OPTIONS,
            ) {
                self.status = picked;
            }
        });
        ui.add_space(12.);
    }

    pub(super) fn sheet(
        &mut self,
        ui: &mut Ui,
        p: &theme::Palette,
        ledger: &crate::listening::Ledger,
        mut texture: impl FnMut(&Ui, f32, Option<&std::sync::Arc<str>>) -> Option<egui::TextureHandle>,
    ) {
        let Some(c) = ledger.collection.as_ref() else {
            ui.label(
                ledger
                    .error
                    .as_deref()
                    .unwrap_or(if ledger.collection_unavailable {
                        "Collection unavailable"
                    } else {
                        "Loading collection…"
                    }),
            );
            return;
        };
        if self.revision != Some(c.revision) {
            self.records = super::collection_gallery::RecordCase::from_collection(c);
            self.artists = c.passes.keys().cloned().collect();
            self.artists
                .sort_by(|a, b| c.passes[b].listen_ms.total_cmp(&c.passes[a].listen_ms));
            self.revision = Some(c.revision);
        }
        ui.horizontal_wrapped(|ui| {
            for (category, label) in [
                (Category::Records, "Records"),
                (Category::Artists, "Artist passes"),
                (Category::Finishes, "App finishes"),
            ] {
                if widgets::chip(ui, p, label, self.category == category).clicked() {
                    self.category = category;
                    self.record_view = RecordView::Case;
                    self.search.clear();
                    self.status = 0;
                }
            }
        });
        ui.add_space(22.);
        match self.category {
            Category::Finishes => {
                finishes(ui, p, ledger.peak_week_minutes, ledger.current_week_minutes)
            }
            Category::Records => {
                if self.records.songs.is_empty() {
                    ui.label(
                        egui::RichText::new("Your first Iron record")
                            .font(theme::semibold(theme::SECTION_PT))
                            .color(p.text),
                    );
                    ui.label("Finish one song 10 times to earn its sleeve.");
                    return;
                }
                if self.record_view == RecordView::Case {
                    let occupied: Vec<_> = SongRank::ALL
                        .into_iter()
                        .rev()
                        .filter(|rank| !self.records.crates[rank.index()].is_empty())
                        .collect();
                    ui.horizontal(|ui| {
                        let label = if occupied.is_empty() {
                            "Your first Iron record"
                        } else {
                            "Record crates"
                        };
                        ui.label(
                            egui::RichText::new(label)
                                .font(theme::semibold(theme::SECTION_PT))
                                .color(p.text),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::icon_button(ui, p, Icon::Search, 17., false)
                                .on_hover_text("Find a record")
                                .clicked()
                            {
                                self.record_view = RecordView::Find;
                                self.search.clear();
                                self.status = 0;
                            }
                        });
                    });
                    ui.add_space(12.);
                    let width = ui.available_width();
                    let gap = 14.;
                    let columns = (((width + gap) / (128. + gap)).floor() as usize).max(1);
                    let tile = ((width - gap * (columns - 1) as f32) / columns as f32).min(165.);
                    for row in occupied.chunks(columns) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for rank in row {
                                let ids = &self.records.crates[rank.index()];
                                let covers = ids
                                    .iter()
                                    .take(3)
                                    .map(|i| {
                                        let song = &c.sleeves[&self.records.songs[*i]];
                                        texture(ui, 120., song.artwork.as_ref())
                                    })
                                    .collect::<Vec<_>>();
                                if super::collection_gallery::crate_tile(
                                    ui,
                                    p,
                                    *rank,
                                    tile,
                                    &covers,
                                    ids.len(),
                                )
                                .clicked()
                                {
                                    if ids.len() == 1 {
                                        self.selected = Some(self.records.songs[ids[0]].clone());
                                    } else {
                                        self.record_view = RecordView::Crate(*rank);
                                    }
                                }
                            }
                        });
                    }
                    if let Some((i, goal)) = self.records.next {
                        if !occupied.is_empty() {
                            ui.add_space(25.);
                            ui.label(
                                egui::RichText::new("Next milestone")
                                    .font(theme::semibold(theme::SECTION_PT))
                                    .color(p.text),
                            );
                        }
                        let key = &self.records.songs[i];
                        let song = &c.sleeves[key];
                        let first_sleeve = occupied.is_empty();
                        let tex = texture(
                            ui,
                            if first_sleeve { 140. } else { 56. },
                            song.artwork.as_ref(),
                        );
                        if super::collection_gallery::next_record(
                            ui,
                            p,
                            song,
                            goal,
                            first_sleeve,
                            tex.as_ref(),
                        )
                        .clicked()
                        {
                            self.selected = Some(key.clone());
                        }
                    }
                } else {
                    let view = self.record_view;
                    ui.horizontal(|ui| {
                        if widgets::icon_button(ui, p, Icon::ChevronLeft, 18., false)
                            .on_hover_text("Record crates")
                            .clicked()
                        {
                            self.record_view = RecordView::Case;
                            self.search.clear();
                            self.status = 0;
                        }
                        let title = match view {
                            RecordView::Crate(rank) => format!("{} crate", rank.name()),
                            _ => "Find a record".into(),
                        };
                        ui.label(
                            egui::RichText::new(title)
                                .font(theme::semibold(theme::SECTION_PT))
                                .color(p.text),
                        );
                    });
                    ui.add_space(8.);
                    if view == RecordView::Find {
                        self.filters(ui, p, "Find a song or artist");
                    }
                    if let RecordView::Crate(rank) = view {
                        let ids = &self.records.crates[rank.index()];
                        let width = ui.available_width();
                        let columns = ((width + 12.) / 152.).floor().max(1.) as usize;
                        let tile =
                            ((width - 12. * (columns - 1) as f32) / columns as f32).min(155.);
                        super::virtual_rows(ui, 146., ids.len().div_ceil(columns), |ui, row| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 12.;
                                for &i in ids.iter().skip(row * columns).take(columns) {
                                    let key = &self.records.songs[i];
                                    let song = &c.sleeves[key];
                                    let tex = texture(ui, 130., song.artwork.as_ref());
                                    if super::record_display::sleeve_tile(
                                        ui,
                                        p,
                                        rank,
                                        song,
                                        tex.as_ref(),
                                        tile,
                                    )
                                    .clicked()
                                    {
                                        self.selected = Some(key.clone());
                                    }
                                }
                            });
                        });
                    } else {
                        // Search includes every sleeve, including songs still earning Iron.
                        let query = self.search.trim().to_lowercase();
                        let keys: Vec<_> = self
                            .records
                            .songs
                            .iter()
                            .filter(|key| {
                                let song = &c.sleeves[*key];
                                (self.status == 0
                                    || (self.status == 1)
                                        == SongRank::from_completed(song.completed).is_some())
                                    && (query.is_empty()
                                        || song.title.to_lowercase().contains(&query)
                                        || song.artist.to_lowercase().contains(&query))
                            })
                            .collect();
                        if keys.is_empty() {
                            ui.label("No matching records");
                        }
                        super::virtual_rows(ui, theme::ROW_HEIGHT, keys.len(), |ui, i| {
                            let key = keys[i];
                            let song = &c.sleeves[key];
                            let tex = texture(ui, 40., song.artwork.as_ref());
                            if super::collection_gallery::record(ui, p, song, tex.as_ref())
                                .clicked()
                            {
                                self.selected = Some(key.clone());
                            }
                        });
                    }
                }
                if let Some(key) = self.selected.as_ref()
                    && let Some(song) = c.sleeves.get(key)
                {
                    let tex = texture(ui, 420., song.artwork.as_ref());
                    if super::record_display::inspect(ui.ctx(), p, key, song, tex.as_ref()) {
                        self.selected = None;
                    }
                }
            }
            Category::Artists => {
                if self.artists.is_empty() {
                    ui.label("Hear 10 songs by one artist or listen on 5 days to earn a pass.");
                    return;
                }
                if self.artists.len() > 12 {
                    self.filters(ui, p, "Find an artist");
                }
                let query = self.search.trim().to_lowercase();
                let keys: Vec<_> = self
                    .artists
                    .iter()
                    .filter(|id| {
                        let pass = &c.passes[*id];
                        (self.status == 0 || pass.stamps.is_empty() == (self.status == 2))
                            && (query.is_empty() || pass.name.to_lowercase().contains(&query))
                    })
                    .collect();
                if keys.is_empty() {
                    ui.label("No matching artists");
                    return;
                }
                super::virtual_rows(ui, theme::ROW_HEIGHT, keys.len(), |ui, i| {
                    let pass = &c.passes[keys[i]];
                    let tier = pass.stamps.len();
                    let (r, hit) = ui.allocate_exact_size(
                        vec2(ui.available_width(), theme::ROW_HEIGHT),
                        if tier > 0 {
                            Sense::click()
                        } else {
                            Sense::hover()
                        },
                    );
                    let art =
                        Rect::from_center_size(pos2(r.left() + 22., r.center().y), vec2(42., 25.));
                    if tier > 0 {
                        ticket(ui, art, tier, p.panel);
                    } else {
                        Icon::Music
                            .image(p.dim, 20.)
                            .paint_at(ui, Rect::from_center_size(art.center(), vec2(20., 20.)));
                    }
                    let x = art.right() + 12.;
                    ui.painter().text(
                        pos2(x, r.center().y - 9.),
                        Align2::LEFT_CENTER,
                        widgets::truncate_to(ui, &pass.name, theme::semibold(14.), r.right() - x),
                        theme::semibold(14.),
                        p.text,
                    );
                    let status = if tier > 0 {
                        TIERS[tier - 1].to_owned()
                    } else {
                        format!("{}/10 songs · {}/5 days", pass.songs(), pass.days())
                    };
                    ui.painter().text(
                        pos2(x, r.center().y + 11.),
                        Align2::LEFT_CENTER,
                        status,
                        theme::regular(12.),
                        p.secondary,
                    );
                    pass_details(ui, hit, p, pass);
                });
            }
        }
    }
}
