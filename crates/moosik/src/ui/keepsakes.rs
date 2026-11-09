//! Small player artwork and the artist-pass sheet. All coordinates are logical points.
use crate::listening::collection::{ArtistPass, TIERS};
use crate::{theme, widgets};
use egui::{Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

const PAPER: Color32 = Color32::from_rgb(239, 222, 180);
const GOLD: Color32 = Color32::from_rgb(226, 185, 94);
const INK: Color32 = Color32::from_rgb(60, 45, 33);
const STRIPE: Color32 = Color32::from_rgb(159, 74, 61);

pub fn sleeve(ui: &Ui, cover: Rect, plays: u64) {
    if plays < 5 {
        return;
    }
    let painter = ui.painter().with_clip_rect(cover.intersect(ui.clip_rect()));
    let scale = cover.width() / 56.;
    let point = |x, y| cover.left_top() + vec2(x, y) * scale;
    // A few exposed paper fibres along the upper right corner of the jacket.
    for (a, b) in [
        ((42., 2.), (49., 2.)),
        ((49., 2.), (53., 6.)),
        ((53., 7.), (53.5, 10.)),
    ] {
        painter.line_segment([point(a.0, a.1), point(b.0, b.1)], Stroke::new(1.4, PAPER));
    }
    if plays >= 15 {
        // Broken arcs, like a record rubbing through the printed ink.
        for (start, end) in [(0.25, 1.65), (1.73, 2.72), (3.18, 4.44), (4.73, 5.8)] {
            let arc = (0..=20)
                .map(|i| {
                    let a = start + (end - start) * i as f32 / 20.;
                    cover.center() + egui::Vec2::angled(a) * (22.5 * scale)
                })
                .collect::<Vec<_>>();
            painter.add(Shape::line(
                arc.clone(),
                Stroke::new(1.55, Color32::from_black_alpha(38)),
            ));
            painter.add(Shape::line(
                arc,
                Stroke::new(0.9, Color32::from_white_alpha(115)),
            ));
        }
    }
    if plays >= 30 {
        let corners = [(39., 3.), (44., 1.), (55., 12.), (52., 17.), (50., 16.)];
        painter.add(Shape::convex_polygon(
            corners.map(|(x, y)| point(x, y)).to_vec(),
            Color32::from_rgba_unmultiplied(227, 203, 150, 235),
            Stroke::new(0.5, INK.gamma_multiply(0.45)),
        ));
        painter.line_segment(
            [point(43., 4.), point(52., 13.)],
            Stroke::new(0.75, Color32::from_white_alpha(90)),
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
    let height = if back { 42. + 34. * tier as f32 } else { 83. };
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
        write(17., &pass.name, theme::semibold(15.));
        for (i, stamp) in pass.stamps.iter().enumerate() {
            let y = 40. + 34. * i as f32;
            write(
                y,
                &format!("{} · {}", TIERS[i], stamp.day),
                theme::medium(11.),
            );
            write(y + 15., &stamp.song, theme::regular(12.));
        }
    } else {
        write(15., TIERS[tier - 1], theme::semibold(11.));
        write(38., &pass.name, theme::bold(20.));
        let progress = pass.next_days().map_or_else(
            || format!("{} listening days", pass.days()),
            |next| {
                format!(
                    "{} more {} until {}",
                    next - pass.days(),
                    if next - pass.days() == 1 {
                        "day"
                    } else {
                        "days"
                    },
                    TIERS[tier]
                )
            },
        );
        write(62., &progress, theme::regular(11.));
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("Flip {}’s {} pass", pass.name, TIERS[tier - 1]),
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
            format!("{} artist pass", pass.name),
        )
    });
    let popup = egui::Popup::menu(&hit)
        .align(egui::RectAlign::TOP_START)
        .gap(5.)
        .frame(egui::Frame::NONE)
        .width(236.);
    if !popup.is_open() {
        hit.clone().on_hover_ui(|ui| {
            pass_card(ui, pass, 236., false, ui.visuals().extreme_bg_color);
        });
    }
    popup.show(|ui| {
        pass_card(ui, pass, 236., true, palette.panel);
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
            "Admit One · {}/10 songs or {}/5 days",
            pass.songs(),
            pass.days()
        ),
        4 => format!("All Access · {} listening days", pass.days()),
        _ => format!(
            "{} earned · {}/{} days to {}",
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
            .on_hover_text("See stamps");
        egui::Popup::menu(&click)
            .frame(egui::Frame::NONE)
            .show(|ui| {
                pass_card(ui, pass, 236., true, p.panel);
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
fn song_row(
    ui: &mut Ui,
    p: &theme::Palette,
    song: &crate::listening::collection::Sleeve,
    paint_cover: impl FnOnce(&Ui, Rect, Option<&std::sync::Arc<str>>),
) {
    let n = song.completed;
    let detail = match n {
        0..5 => format!("Worn corner · {n}/5 full listens"),
        5..15 => format!("Worn corner earned · {n}/15 listens to Record ring"),
        15..30 => format!("Record ring earned · {n}/30 listens to Tape"),
        _ => format!("Tape earned · {n} full listens"),
    };
    let title = if song.artist.is_empty() {
        song.title.clone()
    } else {
        format!("{} · {}", song.title, song.artist)
    };
    reward_row(ui, p, &title, &detail, |ui, t| {
        paint_cover(ui, t, song.artwork.as_ref());
        sleeve(ui, t, n);
    });
}
impl Shelf {
    pub(super) fn sheet(
        &mut self,
        ui: &mut Ui,
        p: &theme::Palette,
        ledger: &crate::listening::Ledger,
        mut paint_cover: impl FnMut(&Ui, Rect, Option<&std::sync::Arc<str>>),
    ) {
        use crate::app_icon::AppIcon;
        let peak = ledger.peak_week_minutes;
        let Some(c) = ledger.collection.as_ref() else {
            let error = ledger.error.as_deref().or(ledger
                .collection_unavailable
                .then_some("Collection unavailable for this listening history."));
            ui.label(error.unwrap_or("Loading collection…"));
            return;
        };
        if self.revision != Some(c.revision) {
            self.artists = by_time(c.passes.iter().map(|(k, v)| (k, v.listen_ms)));
            self.songs = by_time(c.sleeves.iter().map(|(k, v)| (k, v.listen_ms)));
            self.revision = Some(c.revision);
        }
        for (section, label) in [
            (Category::Icons, "App icons"),
            (Category::Artists, "Artist passes"),
            (Category::Songs, "Song sleeves"),
        ] {
            let open = self.browse == Some(section);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(label)
                        .font(theme::semibold(17.))
                        .color(p.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::outline_button(ui, p, if open { "Close" } else { "Browse" })
                        .clicked()
                    {
                        self.browse = if open { None } else { Some(section) };
                        self.search.clear();
                    }
                });
            });
            let open = self.browse == Some(section);
            if open && section != Category::Icons {
                let w = ui.available_width().min(360.);
                widgets::text_input(
                    ui,
                    p,
                    &mut self.search,
                    if section == Category::Artists {
                        "Find an artist"
                    } else {
                        "Find a song or artist"
                    },
                    w,
                );
            }
            match section {
                Category::Icons => {
                    let finishes: Vec<_> = AppIcon::ALL
                        .into_iter()
                        .filter_map(|i| Some((i, i.milestone()?)))
                        .collect();
                    if let Some(peak) = peak {
                        let next = finishes.iter().position(|(_, h)| peak < h * 60);
                        let current = next
                            .unwrap_or(finishes.len())
                            .checked_sub(1)
                            .map(|i| finishes[i].0);
                        for (i, (icon, h)) in finishes.iter().enumerate() {
                            if !open && i != next.unwrap_or(finishes.len() - 1) {
                                continue;
                            }
                            let detail = if peak >= h * 60 {
                                format!("Earned · {h}h listening week")
                            } else {
                                let earned = current.map_or(String::new(), |icon| {
                                    format!("{} earned · ", icon.label())
                                });
                                format!("{earned}Best week {}h {:02}m / {h}h", peak / 60, peak % 60)
                            };
                            reward_row(ui, p, icon.label(), &detail, |ui, t| {
                                if let Some(tex) = icon.texture(ui.ctx()) {
                                    ui.painter().image(
                                        tex.id(),
                                        t,
                                        Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)),
                                        Color32::WHITE,
                                    );
                                }
                            });
                        }
                    } else {
                        ui.label("Loading best week…");
                    }
                }
                Category::Artists | Category::Songs => {
                    let artists = section == Category::Artists;
                    let ranked = if artists { &self.artists } else { &self.songs };
                    let query = self.search.trim().to_lowercase();
                    let keys: Vec<_> = if !open {
                        ranked.iter().take(1).collect()
                    } else {
                        ranked
                            .iter()
                            .filter(|k| {
                                if artists {
                                    let pass = &c.passes[*k];
                                    if query.is_empty() {
                                        !pass.stamps.is_empty()
                                    } else {
                                        pass.name.to_lowercase().contains(&query)
                                    }
                                } else {
                                    let s = &c.sleeves[*k];
                                    if query.is_empty() {
                                        s.completed >= 5
                                    } else {
                                        s.title.to_lowercase().contains(&query)
                                            || s.artist.to_lowercase().contains(&query)
                                    }
                                }
                            })
                            .collect()
                    };
                    if keys.is_empty() {
                        let text = if !open && artists {
                            "Admit One · 10 songs by one artist or 5 listening days"
                        } else if !open {
                            "Worn corner · 5 full listens to one song"
                        } else if !query.is_empty() {
                            "No matches in your listening history"
                        } else {
                            "Nothing earned yet. Search to see progress."
                        };
                        ui.label(egui::RichText::new(text).color(p.secondary));
                    }
                    super::virtual_rows(ui, theme::ROW_HEIGHT, keys.len(), |ui, i| {
                        let k = keys[i];
                        ui.push_id(k, |ui| {
                            if artists {
                                artist_row(ui, p, &c.passes[k])
                            } else {
                                song_row(ui, p, &c.sleeves[k], &mut paint_cover)
                            }
                        });
                    });
                }
            }
            ui.add_space(12.);
        }
    }
}
