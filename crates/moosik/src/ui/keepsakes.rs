//! Small player artwork and the artist-pass sheet. All coordinates are logical points.
use crate::listening::collection::{ArtistPass, Collection, TIERS};
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
    let point = |x, y| cover.left_top() + vec2(x, y);
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
                    cover.center() + egui::Vec2::angled(a) * 22.5
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

pub fn sheet(
    ui: &mut Ui,
    p: &theme::Palette,
    collection: Option<&Collection>,
    error: Option<&str>,
) {
    let Some(collection) = collection else {
        ui.label(error.unwrap_or("Loading passes…"));
        return;
    };
    let mut passes: Vec<_> = collection
        .passes
        .iter()
        .filter(|(_, p)| !p.stamps.is_empty())
        .collect();
    if passes.is_empty() {
        ui.label(egui::RichText::new("Your first artist pass").font(theme::semibold(18.)));
        ui.label("Listen to 10 different songs by one artist, or return on 5 different days.");
        return;
    }
    passes.sort_by(|a, b| a.1.name.cmp(&b.1.name).then(a.0.cmp(b.0)));
    let columns = ((ui.available_width() + 10.) / 230.).floor().max(1.) as usize;
    let width = ((ui.available_width() - 10. * (columns - 1) as f32) / columns as f32).min(290.);
    for row in passes.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 10.;
            for (key, pass) in row {
                let id = ui.id().with(("pass-back", key));
                let back = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
                if pass_card(ui, pass, width, back, p.panel)
                    .on_hover_text("Flip pass")
                    .clicked()
                {
                    ui.data_mut(|d| d.insert_temp(id, !back));
                }
            }
        });
        ui.add_space(8.);
    }
}
