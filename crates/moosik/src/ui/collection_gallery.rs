//! Album artwork and lifetime listening totals for the Records tab.
use crate::listening::{
    awards::SongRank,
    collection::{Collection, Sleeve},
};
use crate::{theme, widgets};
use egui::{Align2, Color32, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

pub(super) fn elapsed(ms: f64) -> String {
    let seconds = (ms.max(0.) / 1000.).round() as u64;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}

pub(super) fn song_stats(ui: &mut Ui, p: &theme::Palette, song: Option<&Sleeve>, duration: f64) {
    let listens = song.map_or(0, |s| s.completed);
    let spent = elapsed(song.map_or(0., |s| s.listen_ms));
    if let Some(rank) = SongRank::from_completed(listens) {
        ui.label(egui::RichText::new(rank.name()).color(super::keepsakes::rank_color(rank)));
    }
    ui.label(format!(
        "{listens} full {} · {spent} listened",
        if listens == 1 { "listen" } else { "listens" }
    ));
    if let Some(next) = SongRank::next_goal(listens) {
        let remaining = next.threshold() - listens;
        let estimate = if duration > 0. && duration.is_finite() {
            format!(" · ≈{}", elapsed(remaining as f64 * duration * 1000.))
        } else {
            String::new()
        };
        ui.label(
            egui::RichText::new(format!(
                "{remaining} {} to {}{estimate}",
                if remaining == 1 { "listen" } else { "listens" },
                next.name()
            ))
            .color(p.secondary),
        );
    }
}

#[derive(Default)]
pub(super) struct RecordCase {
    pub songs: Vec<String>,
    pub crates: [Vec<usize>; 7],
    pub next: Option<(usize, SongRank)>,
}

impl RecordCase {
    pub fn from_collection(collection: &Collection) -> Self {
        let mut case = Self::default();
        case.songs = collection.sleeves.keys().cloned().collect();
        case.songs.sort_by_cached_key(|id| {
            let s = &collection.sleeves[id];
            (s.artist.to_lowercase(), s.title.to_lowercase(), id.clone())
        });
        let mut nearest = None;
        for (index, id) in case.songs.iter().enumerate() {
            let song = &collection.sleeves[id];
            if let Some(rank) = SongRank::from_completed(song.completed) {
                case.crates[rank.index()].push(index);
            }
            if let Some(goal) = SongRank::next_goal(song.completed) {
                let distance = (goal.threshold() - song.completed, std::cmp::Reverse(goal));
                if nearest.is_none_or(|best| distance < best) {
                    nearest = Some(distance);
                    case.next = Some((index, goal));
                }
            }
        }
        case
    }
}

/// An open record bin. The covers visible above the front are from this tier.
pub(super) fn crate_tile(
    ui: &mut Ui,
    p: &theme::Palette,
    rank: SongRank,
    width: f32,
    covers: &[Option<egui::TextureHandle>],
    count: usize,
) -> egui::Response {
    let (r, hit) = ui.allocate_exact_size(vec2(width, width.min(151.)), Sense::click());
    let light = super::keepsakes::rank_color(rank);
    let painter = ui.painter();
    let size = width.min(151.) / 151.;
    let at = |x: f32, y: f32| pos2(r.center().x + (x - 75.) * size, r.top() + y * size);
    let quad = |coords: [(f32, f32); 4], color: Color32| {
        painter.add(Shape::convex_polygon(
            coords.map(|(x, y)| at(x, y)).to_vec(),
            color,
            Stroke::NONE,
        ));
    };
    // The back and side walls reveal the depth of the bin behind its sleeves.
    quad(
        [(8., 58.), (32., 38.), (144., 40.), (122., 63.)],
        widgets::mix(p.panel, light, 0.27),
    );
    quad(
        [(10., 59.), (31., 42.), (140., 44.), (120., 63.)],
        widgets::mix(p.window, Color32::BLACK, 0.25),
    );
    let lift = if hit.hovered() || hit.has_focus() {
        4.
    } else {
        0.
    };
    for (i, cover) in covers.iter().enumerate().rev() {
        let depth = i as f32;
        let x = 19. + depth * 8.;
        let y = 18. - depth * 7. - if i == 0 { lift } else { 0. };
        let corner = [
            at(x, y),
            at(x + 94., y + 2.),
            at(x + 99., y + 95.),
            at(x + 5., y + 92.),
        ];
        painter.add(Shape::convex_polygon(
            corner.to_vec(),
            widgets::mix(p.surface, light, 0.22),
            Stroke::new(1., p.window),
        ));
        if let Some(tex) = cover {
            let mut mesh = egui::Mesh::with_texture(tex.id());
            for (pos, uv) in
                corner
                    .into_iter()
                    .zip([pos2(0., 0.), pos2(1., 0.), pos2(1., 1.), pos2(0., 1.)])
            {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos,
                    uv,
                    color: Color32::from_gray(255 - i as u8 * 14),
                });
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            painter.add(mesh);
        } else {
            let center = at(x + 47., y + 45.);
            for radius in [22., 18., 7.] {
                painter.circle_stroke(center, radius * size, Stroke::new(1., light));
            }
        }
    }
    quad(
        [(122., 66.), (144., 43.), (142., 103.), (120., 123.)],
        widgets::mix(p.window, light, 0.25),
    );
    quad(
        [(7., 64.), (122., 66.), (120., 123.), (11., 120.)],
        widgets::mix(p.surface, light, 0.18),
    );
    quad(
        [(7., 63.), (122., 65.), (122., 68.), (7., 66.)],
        widgets::mix(p.surface, light, 0.63),
    );
    quad(
        [(122., 65.), (144., 42.), (144., 45.), (122., 68.)],
        widgets::mix(p.surface, light, 0.48),
    );
    // Pressed ridges and a lifted bottom edge catch the tier color in grazing light.
    for level in [73., 109.] {
        quad(
            [
                (10., level),
                (121., level + 2.),
                (121., level + 4.),
                (10., level + 2.),
            ],
            widgets::mix(p.surface, light, 0.29),
        );
        quad(
            [
                (121., level + 2.),
                (143., level - 20.),
                (143., level - 18.),
                (121., level + 4.),
            ],
            widgets::mix(p.window, light, 0.38),
        );
    }
    let name = rank.name();
    let font = theme::semibold((11. * size).max(9.));
    let plate = Rect::from_center_size(
        at(65., 94.),
        vec2(
            widgets::text_width(ui, name, &font) + 19. * size,
            22. * size,
        ),
    );
    painter.rect_filled(plate, 2., light);
    painter.text(
        plate.center(),
        Align2::CENTER_CENTER,
        name,
        font,
        Color32::from_rgb(31, 30, 35),
    );
    for x in [plate.left() + 4. * size, plate.right() - 4. * size] {
        painter.circle_filled(
            pos2(x, plate.center().y),
            1.1 * size,
            widgets::mix(light, p.window, 0.55),
        );
    }
    let count = if count == 1 {
        "1 record".into()
    } else {
        format!("{count} records")
    };
    painter.text(
        at(69., 141.),
        Align2::CENTER_CENTER,
        &count,
        theme::regular(12.),
        p.secondary,
    );
    hit.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("{} crate, {count}", rank.name()),
        )
    });
    hit.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(super) fn next_record(
    ui: &mut Ui,
    p: &theme::Palette,
    song: &Sleeve,
    goal: SongRank,
    first_sleeve: bool,
    tex: Option<&egui::TextureHandle>,
) -> egui::Response {
    let width = ui.available_width().min(490.);
    let preview = first_sleeve && width >= 240.;
    let height = if preview { 140. } else { 78. };
    let (r, hit) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if hit.hovered() || hit.has_focus() {
        ui.painter().rect_filled(r, 8., p.surface_hover);
    }
    let art = if preview {
        let art = Rect::from_min_size(r.min, vec2(140., height));
        super::record_display::first_sleeve(ui, p, art, tex);
        art
    } else {
        let art = Rect::from_center_size(pos2(r.left() + 28., r.center().y), vec2(56., 56.));
        widgets::paint_art(ui, p, art, tex, widgets::ArtShape::Square);
        art
    };
    let top = r.center().y - 39.;
    let x = art.right() + 14.;
    let available = r.right() - x - 8.;
    let title = widgets::truncate_to(ui, &song.title, theme::semibold(15.), available);
    ui.painter().text(
        pos2(x, top + 18.),
        Align2::LEFT_CENTER,
        title,
        theme::semibold(15.),
        p.text,
    );
    let left = goal.threshold() - song.completed;
    let label = if preview {
        format!("{} of {} full listens", song.completed, goal.threshold())
    } else {
        format!(
            "{left} {} to {}",
            if left == 1 { "listen" } else { "listens" },
            goal.name()
        )
    };
    ui.painter().text(
        pos2(x, top + 40.),
        Align2::LEFT_CENTER,
        label,
        theme::regular(12.),
        p.secondary,
    );
    let rail = Rect::from_min_size(pos2(x, top + 58.), vec2(available.min(250.), 3.));
    ui.painter().rect_filled(rail, 2., p.surface_active);
    let fill = rail.width() * song.completed as f32 / goal.threshold() as f32;
    if fill > 0. {
        ui.painter().rect_filled(
            Rect::from_min_size(rail.min, vec2(fill, 3.)),
            2.,
            super::keepsakes::rank_color(goal),
        );
    }
    hit.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(super) fn record(
    ui: &mut Ui,
    p: &theme::Palette,
    song: &Sleeve,
    texture: Option<&egui::TextureHandle>,
) -> egui::Response {
    let (r, hit) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::ROW_HEIGHT),
        Sense::click(),
    );
    if hit.hovered() || hit.has_focus() {
        ui.painter().rect_filled(r, 6., p.surface_hover);
    }
    let cy = r.center().y;
    let art = Rect::from_center_size(pos2(r.left() + 21., cy), vec2(40., 40.));
    widgets::paint_art(ui, p, art, texture, widgets::ArtShape::Square);
    super::keepsakes::rank_badge(ui, art, song.completed, None);
    let x = art.right() + 12.;
    let mut text_right = r.right() - 58.;
    if r.width() > 480.
        && let Some(rank) = SongRank::from_completed(song.completed)
    {
        let font = theme::medium(12.);
        ui.painter().text(
            pos2(text_right, cy),
            Align2::RIGHT_CENTER,
            rank.name(),
            font.clone(),
            super::keepsakes::rank_color(rank),
        );
        text_right -= widgets::text_width(ui, rank.name(), &font) + 12.;
    }
    for (value, y, font, color) in [
        (&song.title, cy - 9., theme::semibold(14.), p.text),
        (&song.artist, cy + 10., theme::regular(12.), p.secondary),
    ] {
        ui.painter().text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            widgets::truncate_to(ui, value, font.clone(), (text_right - x).max(0.)),
            font,
            color,
        );
    }
    ui.painter().text(
        pos2(r.right() - 4., cy),
        Align2::RIGHT_CENTER,
        song.completed.to_string(),
        theme::semibold(14.),
        p.text,
    );
    hit.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("Inspect {}", song.title),
        )
    });
    hit.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_case_groups_earned_sleeves_and_tracks_the_closest_promotion() {
        let mut collection = Collection::default();
        assert!(RecordCase::from_collection(&collection).next.is_none());
        for (id, completed) in [("gold", 112), ("new", 9), ("diamond", 1000), ("bronze", 49)] {
            collection.sleeves.insert(
                id.into(),
                Sleeve {
                    title: id.into(),
                    completed,
                    ..Sleeve::default()
                },
            );
        }
        let case = RecordCase::from_collection(&collection);
        assert_eq!(case.songs, ["bronze", "diamond", "gold", "new"]);
        assert_eq!(case.crates[SongRank::Bronze.index()], [0]);
        assert_eq!(case.crates.iter().map(Vec::len).sum::<usize>(), 3);
        assert_eq!(case.next, Some((0, SongRank::Silver)));
        collection.sleeves.get_mut("bronze").unwrap().completed += 1;
        let promoted = RecordCase::from_collection(&collection);
        assert!(promoted.crates[SongRank::Bronze.index()].is_empty());
        assert_eq!(promoted.crates[SongRank::Silver.index()], [0]);
        assert_eq!(promoted.next, Some((3, SongRank::Iron)));
    }
}
