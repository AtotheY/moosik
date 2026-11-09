//! Dimensional song display for the Collection shelf.
//!
//! The real album art is painted into the front face by the caller. The bevel,
//! pedestal, and optional crystal are native shapes so they work for any cover
//! without shipping a fixed illustration of somebody else's song.

use std::sync::Arc;

use crate::listening::awards::SongRank;
use crate::{theme, widgets};
use egui::{Color32, Pos2, Rect, Shape, Stroke, Ui, pos2, vec2};

/// The material hue also used by the badge on song artwork.
pub(super) fn metal(rank: SongRank) -> Color32 {
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

fn facet(ui: &Ui, vertices: &[Pos2], fill: Color32) {
    ui.painter()
        .add(Shape::convex_polygon(vertices.to_vec(), fill, Stroke::NONE));
}

fn ellipse(ui: &Ui, center: Pos2, rx: f32, ry: f32, fill: Color32) {
    let vertices: Vec<_> = (0..32)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 32.;
            pos2(center.x + a.cos() * rx, center.y + a.sin() * ry)
        })
        .collect();
    ui.painter()
        .add(Shape::convex_polygon(vertices, fill, Stroke::NONE));
}

fn sparkle(ui: &Ui, center: Pos2, size: f32, color: Color32) {
    // Two convex glints avoid feeding a concave star to egui's convex tessellator.
    ui.painter().add(Shape::convex_polygon(
        vec![
            center + vec2(0., -size),
            center + vec2(size * 0.17, 0.),
            center + vec2(0., size),
            center + vec2(-size * 0.17, 0.),
        ],
        color,
        Stroke::NONE,
    ));
    ui.painter().add(Shape::convex_polygon(
        vec![
            center + vec2(-size, 0.),
            center + vec2(0., -size * 0.17),
            center + vec2(size, 0.),
            center + vec2(0., size * 0.17),
        ],
        color,
        Stroke::NONE,
    ));
    ui.painter()
        .circle_filled(center, (size * 0.2).max(0.5), Color32::WHITE);
}

/// Paint a full exhibit, including the dynamic cover and its tier material.
///
/// `stage` is the entire display area, not just the cover. Returns the front
/// cover rect so the caller can place a small rank badge on its bottom-left.
pub(super) fn paint(
    ui: &Ui,
    palette: &theme::Palette,
    stage: Rect,
    artwork: Option<&Arc<str>>,
    rank: Option<SongRank>,
    paint_cover: &mut impl FnMut(&Ui, Rect, Option<&Arc<str>>),
) -> Rect {
    let w = stage.width().max(1.);
    let h = stage.height().max(1.);
    let is_jewel = rank == Some(SongRank::Diamond);
    let color = rank.map_or(palette.dim, metal);
    let dim = widgets::mix(
        palette.panel,
        color,
        if rank.is_some() { 0.53 } else { 0.25 },
    );
    let bright = widgets::mix(color, Color32::WHITE, if is_jewel { 0.42 } else { 0.27 });

    // A bounded, static halo makes the high tiers feel lit without idle animation.
    if is_jewel {
        let halo = pos2(stage.center().x, stage.top() + h * 0.47);
        let glow = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 18);
        for (radius, strength) in [(0.50, 0.42), (0.38, 0.66), (0.26, 1.0)] {
            ui.painter()
                .circle_filled(halo, w.min(h) * radius, glow.gamma_multiply(strength));
        }
    }

    let cover_side = (w * 0.57)
        .min(h * if is_jewel { 0.63 } else { 0.64 })
        .max(1.);
    let depth = (cover_side * 0.065).clamp(3., 11.);
    let top = stage.top() + h * 0.055;
    let front = Rect::from_min_size(
        pos2(stage.center().x - cover_side * 0.5 - depth * 0.3, top),
        vec2(cover_side, cover_side),
    );
    let back = front.translate(vec2(depth, depth * 0.7));
    let shadow = back.translate(vec2(depth * 0.45, depth * 0.9));
    ui.painter().rect_filled(
        shadow,
        theme::RADIUS,
        Color32::from_black_alpha(if palette.dark { 125 } else { 50 }),
    );

    // The extruded top and right edges read as a thick, tilted album slab.
    facet(
        ui,
        &[
            front.left_top(),
            back.left_top(),
            back.right_top(),
            front.right_top(),
        ],
        bright,
    );
    facet(
        ui,
        &[
            front.right_top(),
            back.right_top(),
            back.right_bottom(),
            front.right_bottom(),
        ],
        dim,
    );
    ui.painter()
        .rect_filled(front, theme::RADIUS, palette.surface);
    paint_cover(ui, front.shrink(1.5), artwork);
    ui.painter().add(Shape::convex_polygon(
        vec![
            front.left_top(),
            pos2(front.left() + cover_side * 0.37, front.top()),
            pos2(front.left(), front.top() + cover_side * 0.49),
        ],
        Color32::from_white_alpha(if palette.dark { 12 } else { 7 }),
        Stroke::NONE,
    ));
    ui.painter().rect_stroke(
        front,
        theme::RADIUS,
        Stroke::new(if is_jewel { 1.7 } else { 1.2 }, bright),
        egui::StrokeKind::Inside,
    );
    ui.painter().vline(
        front.left() + 1.8,
        (front.top() + 5.)..=(front.bottom() - 5.),
        Stroke::new(
            1.,
            Color32::WHITE.gamma_multiply(if palette.dark { 0.43 } else { 0.22 }),
        ),
    );

    // The same footprint fits all tiers. Diamond gets a faceted crystal under
    // the art; the other materials use a short beveled support.
    let base_y = stage.bottom() - (h * 0.095).max(10.);
    let base_rx = cover_side * 0.60;
    let base_ry = (cover_side * 0.048).max(3.5);
    let support_top = front.bottom() + depth * 0.42;
    let support_bottom = base_y - base_ry * 0.6;
    if support_bottom > support_top + 4. {
        if is_jewel {
            let top_half = cover_side * 0.43;
            let shoulder_y = support_top + (support_bottom - support_top) * 0.23;
            let tip = pos2(stage.center().x, support_bottom);
            let lt = pos2(stage.center().x - top_half, support_top);
            let rt = pos2(stage.center().x + top_half, support_top);
            let lm = pos2(stage.center().x - top_half * 0.79, shoulder_y);
            let rm = pos2(stage.center().x + top_half * 0.79, shoulder_y);
            let mid = pos2(
                stage.center().x,
                shoulder_y + (support_bottom - shoulder_y) * 0.29,
            );
            facet(ui, &[lt, rt, rm, mid, lm], widgets::mix(dim, color, 0.45));
            facet(ui, &[lt, lm, tip], widgets::mix(palette.panel, color, 0.68));
            facet(
                ui,
                &[lt, mid, tip],
                widgets::mix(color, Color32::WHITE, 0.28),
            );
            facet(
                ui,
                &[lt, rt, mid],
                widgets::mix(color, Color32::WHITE, 0.61),
            );
            facet(ui, &[rt, rm, tip], widgets::mix(palette.panel, color, 0.43));
            facet(
                ui,
                &[rt, mid, tip],
                widgets::mix(color, Color32::WHITE, 0.11),
            );
        } else {
            let top_half = cover_side * 0.33;
            let bottom_half = cover_side * 0.45;
            let mid_x = stage.center().x;
            facet(
                ui,
                &[
                    pos2(mid_x - top_half, support_top),
                    pos2(mid_x + top_half, support_top),
                    pos2(mid_x + bottom_half, support_bottom),
                    pos2(mid_x - bottom_half, support_bottom),
                ],
                dim,
            );
            facet(
                ui,
                &[
                    pos2(mid_x - top_half, support_top),
                    pos2(mid_x, support_top),
                    pos2(mid_x, support_bottom),
                    pos2(mid_x - bottom_half, support_bottom),
                ],
                widgets::mix(dim, bright, 0.2),
            );
        }
    }

    // A shallow cylinder grounds the object and remains a muted outline for
    // songs that have not yet reached Iron.
    let plinth = pos2(stage.center().x, base_y);
    ellipse(
        ui,
        plinth + vec2(0., base_ry * 1.36),
        base_rx,
        base_ry,
        widgets::mix(
            palette.panel,
            color,
            if rank.is_some() { 0.33 } else { 0.1 },
        ),
    );
    ui.painter().rect_filled(
        Rect::from_min_max(
            pos2(plinth.x - base_rx, plinth.y),
            pos2(plinth.x + base_rx, plinth.y + base_ry * 1.36),
        ),
        2.,
        dim,
    );
    ellipse(
        ui,
        plinth,
        base_rx,
        base_ry,
        widgets::mix(dim, bright, if rank.is_some() { 0.55 } else { 0.18 }),
    );
    ellipse(
        ui,
        plinth - vec2(0., base_ry * 0.12),
        base_rx * 0.78,
        base_ry * 0.47,
        widgets::mix(
            palette.panel,
            color,
            if rank.is_some() { 0.25 } else { 0.08 },
        ),
    );

    if is_jewel {
        for i in 0..5 {
            let (x, y, s) = match i {
                0 => (0.86, 0.22, 0.020),
                1 => (0.14, 0.57, 0.014),
                2 => (0.73, 0.73, 0.010),
                3 => (0.21, 0.19, 0.011),
                _ => (0.92, 0.48, 0.012),
            };
            sparkle(
                ui,
                pos2(stage.left() + w * x, stage.top() + h * y),
                w.min(h) * s,
                bright,
            );
        }
    }

    front
}
