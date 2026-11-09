//! Ranked song trophies inside the lifetime Collection page.
use crate::listening::awards::SongRank;
use crate::listening::collection::Sleeve;
use crate::{theme, widgets};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::sync::Arc;

use super::{keepsakes, record_display};

fn count(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else {
        format!("{},{:03}", n / 1_000, n % 1_000)
    }
}

fn time_spent(ms: f64) -> String {
    let minutes = (ms.max(0.) / 60_000.).floor() as u64;
    if minutes >= 60 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

fn text(ui: &Ui, x: f32, y: f32, value: &str, max: f32, font: egui::FontId, color: Color32) {
    ui.painter().text(
        pos2(x, y),
        Align2::LEFT_CENTER,
        widgets::truncate_to(ui, value, font.clone(), max.max(10.)),
        font,
        color,
    );
}

pub(super) fn ladder(ui: &mut Ui, p: &theme::Palette) {
    let width = ui.available_width();
    let cols = if width >= 1_180. {
        7
    } else if width >= 650. {
        4
    } else if width >= 460. {
        3
    } else {
        2
    };
    let rows = SongRank::ALL.len().div_ceil(cols);
    let height = 36. + rows as f32 * 27. + 8.;
    let (r, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    ui.painter().rect_filled(r, 12., p.surface);
    ui.painter().rect_stroke(
        r,
        12.,
        Stroke::new(0.8, p.surface_active),
        egui::StrokeKind::Inside,
    );
    text(
        ui,
        r.left() + 13.,
        r.top() + 18.,
        "THE SONG LADDER",
        r.width() - 26.,
        theme::semibold(10.),
        p.secondary,
    );
    let cell = (r.width() - 26.) / cols as f32;
    for (i, rank) in SongRank::ALL.iter().copied().enumerate() {
        let x = r.left() + 13. + (i % cols) as f32 * cell;
        let y = r.top() + 43. + (i / cols) as f32 * 27.;
        let color = record_display::metal(rank);
        ui.painter()
            .circle_filled(pos2(x + 7., y), 7., color.gamma_multiply(0.28));
        ui.painter()
            .circle_stroke(pos2(x + 7., y), 6., Stroke::new(1.5, color));
        ui.painter().circle_filled(pos2(x + 7., y), 2.7, color);
        text(
            ui,
            x + 22.,
            y,
            rank.name(),
            cell - 55.,
            theme::semibold(11.),
            p.text,
        );
        ui.painter().text(
            pos2(x + cell - 5., y),
            Align2::RIGHT_CENTER,
            count(rank.threshold()),
            theme::regular(10.),
            p.secondary,
        );
    }
}

fn trophy_card(
    ui: &mut Ui,
    p: &theme::Palette,
    song: &Sleeve,
    ordinal: usize,
    width: f32,
    height: f32,
    paint_cover: &mut impl FnMut(&Ui, Rect, Option<&Arc<str>>),
) {
    let tier = SongRank::from_completed(song.completed);
    let color = tier.map_or(p.secondary, record_display::metal);
    let (r, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let surface = tier.map_or(p.surface, |rank| {
        widgets::mix(
            p.surface,
            record_display::metal(rank),
            if rank >= SongRank::DoublePlatinum {
                0.10
            } else {
                0.045
            },
        )
    });
    ui.painter().rect_filled(r, 12., surface);
    ui.painter().rect_stroke(
        r,
        12.,
        Stroke::new(0.8, widgets::mix(p.surface_active, color, 0.34)),
        egui::StrokeKind::Inside,
    );
    text(
        ui,
        r.left() + 14.,
        r.top() + 17.,
        &format!("#{:02}", ordinal),
        60.,
        theme::bold(14.),
        p.text,
    );
    let label = tier.map_or("TOWARD IRON", SongRank::name).to_uppercase();
    let label_font = theme::semibold(10.);
    let label_width = widgets::text_width(ui, &label, &label_font) + 19.;
    let badge = Rect::from_min_size(
        pos2(r.right() - label_width - 13., r.top() + 7.),
        vec2(label_width, 21.),
    );
    ui.painter()
        .rect_filled(badge, 11., widgets::mix(p.surface, color, 0.12));
    ui.painter().rect_stroke(
        badge,
        11.,
        Stroke::new(0.8, color.gamma_multiply(0.68)),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        label,
        label_font,
        color,
    );
    let exhibit = Rect::from_min_max(
        pos2(r.left() + 18., r.top() + 32.),
        pos2(r.right() - 18., r.bottom() - 116.),
    );
    let cover = record_display::paint(ui, p, exhibit, song.artwork.as_ref(), tier, paint_cover);
    keepsakes::rank_badge(ui, cover, song.completed, Some(ordinal));
    text(
        ui,
        r.left() + 14.,
        r.bottom() - 104.,
        &song.title,
        width - 28.,
        theme::semibold(15.),
        p.text,
    );
    text(
        ui,
        r.left() + 14.,
        r.bottom() - 85.,
        &song.artist,
        width - 28.,
        theme::regular(11.),
        p.secondary,
    );
    ui.painter().hline(
        (r.left() + 14.)..=(r.right() - 14.),
        r.bottom() - 72.,
        Stroke::new(0.7, p.surface_active),
    );
    let right_x = r.left() + width * 0.52;
    text(
        ui,
        r.left() + 14.,
        r.bottom() - 57.,
        &count(song.completed),
        right_x - r.left() - 20.,
        theme::bold(13.),
        p.text,
    );
    text(
        ui,
        right_x,
        r.bottom() - 57.,
        &time_spent(song.listen_ms),
        r.right() - right_x - 14.,
        theme::bold(13.),
        p.text,
    );
    text(
        ui,
        r.left() + 14.,
        r.bottom() - 43.,
        "full listens",
        right_x - r.left() - 20.,
        theme::regular(10.),
        p.secondary,
    );
    text(
        ui,
        right_x,
        r.bottom() - 43.,
        "listening time",
        r.right() - right_x - 14.,
        theme::regular(10.),
        p.secondary,
    );
    let rail = Rect::from_min_size(
        pos2(r.left() + 14., r.bottom() - 28.),
        vec2(width - 28., 3.),
    );
    ui.painter().rect_filled(rail, 2., p.surface_active);
    let next = SongRank::next_goal(song.completed);
    let fraction = next
        .map_or(1., |rank| song.completed as f32 / rank.threshold() as f32)
        .clamp(0., 1.);
    if fraction > 0. {
        ui.painter().rect_filled(
            Rect::from_min_size(rail.min, vec2(rail.width() * fraction, 3.)),
            2.,
            color,
        );
    }
    let progress = next.map_or_else(
        || "Every song tier earned".to_owned(),
        |rank| {
            format!(
                "Next: {} · {} to go",
                rank.name(),
                count(rank.threshold() - song.completed)
            )
        },
    );
    text(
        ui,
        r.left() + 14.,
        r.bottom() - 13.,
        &progress,
        width - 28.,
        theme::regular(10.),
        p.secondary,
    );
    response.on_hover_text(format!(
        "#{ordinal} · {}\n{} · {} full listens · {} listening time",
        tier.map_or("In progress", SongRank::name),
        song.title,
        count(song.completed),
        time_spent(song.listen_ms)
    ));
}

/// Always shows the first three songs in the same order as the full list.
pub(super) fn featured(
    ui: &mut Ui,
    p: &theme::Palette,
    songs: &[(&Sleeve, usize)],
    paint_cover: &mut impl FnMut(&Ui, Rect, Option<&Arc<str>>),
) {
    if songs.is_empty() {
        return;
    }
    let available = ui.available_width();
    let songs = &songs[..songs.len().min(3)];
    if available >= 680. && songs.len() >= 3 {
        let gap = 10.;
        let hero_width = (available - 2. * gap) * 0.38;
        let other_width = (available - 2. * gap - hero_width) * 0.5;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (i, &(song, position)) in songs.iter().enumerate() {
                trophy_card(
                    ui,
                    p,
                    song,
                    position,
                    if i == 0 { hero_width } else { other_width },
                    350.,
                    paint_cover,
                );
            }
        });
    } else if available >= 500. && songs.len() >= 2 {
        trophy_card(ui, p, songs[0].0, songs[0].1, available, 358., paint_cover);
        ui.add_space(10.);
        let width = (available - 10.) / 2.;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.;
            for &(song, position) in &songs[1..] {
                trophy_card(ui, p, song, position, width, 335., paint_cover);
            }
        });
    } else {
        for (i, &(song, position)) in songs.iter().enumerate() {
            if i > 0 {
                ui.add_space(10.);
            }
            trophy_card(ui, p, song, position, available, 350., paint_cover);
        }
    }
}

pub(super) fn compact_row(
    ui: &mut Ui,
    p: &theme::Palette,
    song: &Sleeve,
    ordinal: usize,
    paint_cover: &mut impl FnMut(&Ui, Rect, Option<&Arc<str>>),
) {
    let narrow = ui.available_width() < 520.;
    let height = if narrow { 67. } else { theme::ROW_HEIGHT };
    let (r, hit) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    ui.painter().rect_filled(r, 8., p.surface);
    let rank_x = r.left() + 10.;
    text(
        ui,
        rank_x,
        r.center().y,
        &format!("#{:02}", ordinal),
        27.,
        theme::semibold(11.),
        p.secondary,
    );
    let art = Rect::from_min_size(pos2(r.left() + 43., r.center().y - 20.), vec2(40., 40.));
    paint_cover(ui, art, song.artwork.as_ref());
    keepsakes::rank_badge(ui, art, song.completed, Some(ordinal));
    let x = art.right() + 10.;
    let tier = SongRank::from_completed(song.completed);
    let goal = SongRank::next_goal(song.completed);
    let status = tier.map_or("Toward Iron", SongRank::name);
    let detail = goal.map_or_else(
        || "Every tier earned".to_owned(),
        |rank| {
            format!(
                "{} · {}/{} to {}",
                status,
                count(song.completed),
                count(rank.threshold()),
                rank.name()
            )
        },
    );
    let right_width = if narrow { 0. } else { 150. };
    text(
        ui,
        x,
        r.center().y - 10.,
        &song.title,
        r.right() - x - right_width - 12.,
        theme::semibold(12.),
        p.text,
    );
    let detail_line = if narrow {
        format!("{} · {}", song.artist, detail)
    } else {
        detail
    };
    text(
        ui,
        x,
        r.center().y + 9.,
        &detail_line,
        r.right() - x - right_width - 12.,
        theme::regular(10.),
        p.secondary,
    );
    if narrow {
        text(
            ui,
            x,
            r.center().y + 24.,
            &format!(
                "{} full listens · {} listening time",
                count(song.completed),
                time_spent(song.listen_ms)
            ),
            r.right() - x - 12.,
            theme::regular(10.),
            p.secondary,
        );
    } else {
        ui.painter().text(
            pos2(r.right() - 11., r.center().y - 9.),
            Align2::RIGHT_CENTER,
            format!("{} full listens", count(song.completed)),
            theme::semibold(11.),
            p.text,
        );
        ui.painter().text(
            pos2(r.right() - 11., r.center().y + 9.),
            Align2::RIGHT_CENTER,
            format!("{} listening time", time_spent(song.listen_ms)),
            theme::regular(10.),
            p.secondary,
        );
    }
    hit.on_hover_text(format!(
        "#{ordinal} · {} · {}\n{} full listens · {} listening time",
        song.title,
        tier.map_or("In progress", SongRank::name),
        count(song.completed),
        time_spent(song.listen_ms)
    ));
}
