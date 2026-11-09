//! A textured record jacket, pressed disc and foil seal in a perspective camera.
use super::{collection_gallery, keepsakes};
use crate::{
    icons::Icon,
    listening::{awards::SongRank, collection::Sleeve},
    theme, widgets,
};
use egui::{Color32, Mesh, Painter, Pos2, Sense, Shape, Stroke, Vec2, pos2, vec2};
use std::f32::consts::TAU;

struct Camera {
    at: Pos2,
    scale: f32,
    yaw: f32,
    pitch: f32,
}

impl Camera {
    fn project(&self, x: f32, y: f32, z: f32) -> Pos2 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let depth = z * cy - x * sy;
        let zoom = self.scale * 5.5 / (5.5 - depth * cp - y * sp);
        self.at + vec2(x * cy + z * sy, y * cp - depth * sp) * zoom
    }

    fn polygon(&self, painter: &Painter, points: &[[f32; 3]], fill: Color32) {
        let vertices = points
            .iter()
            .map(|&[x, y, z]| self.project(x, y, z))
            .collect();
        painter.add(Shape::convex_polygon(vertices, fill, Stroke::NONE));
    }

    fn disc_point(&self, radius: f32, angle: f32, z: f32) -> Pos2 {
        self.project(0.49 + radius * angle.cos(), radius * angle.sin(), z)
    }

    fn disc(&self, painter: &Painter, accent: Color32) {
        const COUNT: usize = 120;
        let center = self.disc_point(0., 0., 0.);
        let outer: Vec<_> = (0..COUNT)
            .map(|i| self.disc_point(0.95, TAU * i as f32 / COUNT as f32, 0.))
            .collect();
        // The two broad highlights follow the viewing angle like light on vinyl grooves.
        let mut mesh = Mesh::default();
        mesh.colored_vertex(center, Color32::from_gray(17));
        for (i, &point) in outer.iter().enumerate() {
            let angle = TAU * i as f32 / COUNT as f32;
            let shine = (angle * 2. + self.yaw - 0.55).cos().max(0.).powi(8);
            mesh.colored_vertex(
                point,
                widgets::mix(
                    Color32::from_rgb(18, 19, 29),
                    Color32::from_rgb(158, 155, 167),
                    shine * 0.84 + 0.08,
                ),
            );
            mesh.add_triangle(0, i as u32 + 1, ((i + 1) % COUNT) as u32 + 1);
        }
        painter.add(mesh);
        for n in 0..30 {
            let radius = 0.35 + n as f32 * 0.0205;
            let ring = (0..COUNT)
                .map(|i| self.disc_point(radius, i as f32 * TAU / COUNT as f32, 0.))
                .collect();
            painter.add(Shape::closed_line(
                ring,
                Stroke::new(
                    if n % 6 == 0 { 0.85 } else { 0.45 },
                    Color32::from_white_alpha(if n % 6 == 0 { 60 } else { 32 }),
                ),
            ));
        }
        let label = (0..64)
            .map(|i| self.disc_point(0.29, TAU * i as f32 / 64., 0.))
            .collect();
        painter.add(Shape::convex_polygon(label, accent, Stroke::NONE));
        painter.circle_filled(center, 2.4, Color32::from_gray(23));
    }

    fn cover(
        &self,
        painter: &Painter,
        texture: Option<&egui::TextureHandle>,
        accent: Color32,
        front: bool,
    ) {
        let z = if front { 0.075 } else { -0.075 };
        let base = if front {
            accent
        } else {
            widgets::mix(Color32::from_rgb(20, 21, 27), accent, 0.27)
        };
        self.polygon(
            painter,
            &[
                [-1.32, -1., z],
                [0.68, -1., z],
                [0.68, 1., z],
                [-1.32, 1., z],
            ],
            base,
        );
        if front && let Some(tex) = texture {
            // Subdivide the face so the UV projection stays accurate at oblique angles.
            const N: usize = 12;
            let mut mesh = Mesh::with_texture(tex.id());
            let light = Color32::from_gray((223. + 32. * self.yaw.cos().abs()) as u8);
            for j in 0..=N {
                for i in 0..=N {
                    let u = i as f32 / N as f32;
                    let v = j as f32 / N as f32;
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: self.project(-1.309 + 1.978 * u, -0.989 + 1.978 * v, z),
                        uv: pos2(u, v),
                        color: light,
                    });
                    if i < N && j < N {
                        let a = (j * (N + 1) + i) as u32;
                        let b = a + N as u32 + 1;
                        mesh.add_triangle(a, a + 1, b);
                        mesh.add_triangle(a + 1, b + 1, b);
                    }
                }
            }
            painter.add(mesh);
        } else {
            // Pressed circular foil on the reverse also identifies an artless sleeve.
            for n in 0..8 {
                let radius = 0.42 + n as f32 * 0.043;
                let ring = (0..72)
                    .map(|i| {
                        let a = TAU * i as f32 / 72.;
                        self.project(-0.32 + a.cos() * radius, a.sin() * radius, z)
                    })
                    .collect();
                painter.add(Shape::closed_line(
                    ring,
                    Stroke::new(0.9, accent.gamma_multiply(0.72)),
                ));
            }
        }
    }

    fn seal(&self, painter: &Painter, accent: Color32, diamond: bool) {
        let scale = if diamond { 0.24 } else { 0.15 };
        // The raised crown and pointed pavilion catch different light as the jacket turns.
        let vertices = [
            [-0.95, -0.12, 0.085],
            [-0.57, -0.64, 0.11],
            [0.49, -0.68, 0.11],
            [0.97, -0.12, 0.085],
            [0.03, 1.0, 0.09],
            [-0.36, -0.1, 0.18],
            [0.32, -0.14, 0.19],
            [0.0, -0.59, 0.16],
        ];
        let points = vertices.map(|[x, y, z]| self.project(0.39 + x * scale, 0.69 + y * scale, z));
        let facets = [
            ([0, 1, 5], 0.16),
            ([1, 7, 5], 0.91),
            ([7, 6, 5], 0.61),
            ([7, 2, 6], 0.27),
            ([2, 3, 6], 0.96),
            ([0, 5, 4], 0.38),
            ([5, 6, 4], 0.76),
            ([6, 3, 4], 0.12),
        ];
        for (idx, (corners, base)) in facets.into_iter().enumerate() {
            let shine = (base + 0.18 * (self.yaw + idx as f32).sin()).clamp(0., 1.);
            let tint = widgets::mix(
                widgets::mix(Color32::from_rgb(13, 32, 48), accent, 0.48),
                Color32::WHITE,
                shine,
            );
            painter.add(Shape::convex_polygon(
                corners.map(|i| points[i]).to_vec(),
                tint,
                Stroke::NONE,
            ));
        }
        let at = points[2];
        let arm = 4. + 5. * self.yaw.cos().abs();
        for (w, h) in [(arm, 1.), (1., arm)] {
            painter.add(Shape::convex_polygon(
                vec![
                    at - vec2(w, 0.),
                    at - vec2(0., h),
                    at + vec2(w, 0.),
                    at + vec2(0., h),
                ],
                Color32::WHITE,
                Stroke::NONE,
            ));
        }
    }

    fn draw(
        &self,
        painter: &Painter,
        texture: Option<&egui::TextureHandle>,
        rank: Option<SongRank>,
        accent: Color32,
    ) {
        let front = self.yaw.cos() >= 0.;
        // The disc sits between the front and back boards. Paint the far board first.
        self.cover(painter, texture, accent, !front);
        self.disc(painter, accent);
        let side = if self.yaw.sin() > 0. { -1.32 } else { 0.68 };
        self.polygon(
            painter,
            &[
                [side, -1., -0.075],
                [side, 1., -0.075],
                [side, 1., 0.075],
                [side, -1., 0.075],
            ],
            widgets::mix(Color32::from_gray(52), accent, 0.55),
        );
        let y = if self.pitch.sin() > 0. { 1. } else { -1. };
        self.polygon(
            painter,
            &[
                [-1.32, y, -0.075],
                [0.68, y, -0.075],
                [0.68, y, 0.075],
                [-1.32, y, 0.075],
            ],
            accent.gamma_multiply(0.82),
        );
        self.cover(painter, texture, accent, front);
        if front && rank.is_some() {
            self.seal(painter, accent, rank == Some(SongRank::Diamond));
        }
    }
}

pub(super) fn first_sleeve(
    ui: &egui::Ui,
    p: &theme::Palette,
    rect: egui::Rect,
    texture: Option<&egui::TextureHandle>,
) {
    let camera = Camera {
        at: rect.center(),
        scale: rect.width() * 0.32,
        yaw: -0.42,
        pitch: -0.14,
    };
    camera.draw(
        &ui.painter().with_clip_rect(rect),
        texture,
        None,
        p.secondary,
    );
}

pub(super) fn sleeve_tile(
    ui: &mut egui::Ui,
    p: &theme::Palette,
    rank: SongRank,
    song: &Sleeve,
    texture: Option<&egui::TextureHandle>,
    width: f32,
) -> egui::Response {
    let (rect, hit) = ui.allocate_exact_size(vec2(width, 146.), Sense::click());
    let camera = Camera {
        at: pos2(rect.center().x, rect.top() + 53.),
        scale: width.min(146.) * 0.30,
        yaw: if hit.hovered() { -0.30 } else { -0.47 },
        pitch: -0.14,
    };
    camera.draw(
        &ui.painter().with_clip_rect(rect),
        texture,
        Some(rank),
        keepsakes::rank_color(rank),
    );
    for (value, offset, font, color) in [
        (&song.title, 115., theme::semibold(13.), p.text),
        (&song.artist, 133., theme::regular(12.), p.secondary),
    ] {
        ui.painter().text(
            pos2(rect.left() + 2., rect.top() + offset),
            egui::Align2::LEFT_CENTER,
            widgets::truncate_to(ui, value, font.clone(), width - 4.),
            font,
            color,
        );
    }
    hit.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("Inspect {}", song.title),
        )
    });
    hit.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(super) fn inspect(
    ctx: &egui::Context,
    p: &theme::Palette,
    key: &str,
    song: &Sleeve,
    texture: Option<&egui::TextureHandle>,
) -> bool {
    let id = egui::Id::new(("record-turntable", key));
    let mut close = false;
    let rank = SongRank::from_completed(song.completed);
    let accent = rank.map_or(p.secondary, keepsakes::rank_color);
    let width = (ctx.content_rect().width() - 72.).clamp(235., 470.);
    let stage_height = (ctx.content_rect().height() - 225.).clamp(125., 300.);
    let modal = egui::Modal::new(id)
        .frame(
            egui::Frame::new()
                .fill(p.overlay)
                .inner_margin(18.)
                .corner_radius(15),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                let name = rank.map_or("Record sleeve".to_string(), |r| {
                    format!("{} record", r.name())
                });
                ui.label(
                    egui::RichText::new(name)
                        .font(theme::semibold(13.))
                        .color(accent),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = widgets::icon_button(ui, p, Icon::X, 16., false)
                        .on_hover_text("Close")
                        .clicked();
                });
            });
            let (r, hit) =
                ui.allocate_exact_size(vec2(width, stage_height), Sense::click_and_drag());
            let angles_id = id.with("angles");
            let mut angles = ui
                .data(|data| data.get_temp::<Vec2>(angles_id))
                .unwrap_or(vec2(-0.38, -0.14));
            let delta = hit.drag_delta();
            if hit.clicked() || hit.drag_started() || ui.memory(|m| m.focused().is_none()) {
                hit.request_focus();
            }
            if hit.has_focus() {
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        hit.id,
                        egui::EventFilter {
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            ..Default::default()
                        },
                    )
                });
                ui.input(|i| {
                    if i.key_pressed(egui::Key::ArrowUp) {
                        angles.y += 0.30;
                    }
                    if i.key_pressed(egui::Key::ArrowDown) {
                        angles.y -= 0.30;
                    }
                    if i.key_pressed(egui::Key::ArrowLeft) {
                        angles.x -= TAU / 32.;
                    }
                    if i.key_pressed(egui::Key::ArrowRight) {
                        angles.x += TAU / 32.;
                    }
                });
            }
            angles.x = (angles.x + delta.x * 0.013) % TAU;
            angles.y = (angles.y - delta.y * 0.01).clamp(-0.85, 0.85);
            ui.data_mut(|data| data.insert_temp(angles_id, angles));
            hit.clone().on_hover_cursor(if hit.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
            let painter = ui.painter().with_clip_rect(r);
            let mut camera = Camera {
                at: r.center(),
                scale: (r.width() * 0.30).min(r.height() * 0.38),
                yaw: angles.x,
                pitch: angles.y,
            };
            // Keep the closest corner in frame when a thick, tilted jacket is edge-on.
            let mut extent = Vec2::ZERO;
            for x in [-1.32, 1.44] {
                for y in [-1., 1.] {
                    for z in [-0.075, 0.18] {
                        extent = extent.max((camera.project(x, y, z) - r.center()).abs());
                    }
                }
            }
            camera.scale *= (r.height() * 0.47 / extent.y)
                .min(r.width() * 0.48 / extent.x)
                .min(1.);
            for n in (1..=14).rev() {
                painter.add(Shape::ellipse_filled(
                    pos2(r.center().x, r.bottom() - 10.),
                    vec2(
                        camera.scale * (0.50 + n as f32 * 0.05),
                        2. + n as f32 * 0.52,
                    ),
                    Color32::from_black_alpha(7),
                ));
            }
            camera.draw(&painter, texture, rank, accent);
            ui.add_space(4.);
            ui.label(
                egui::RichText::new(&song.title)
                    .font(theme::bold(20.))
                    .color(p.text),
            );
            ui.label(egui::RichText::new(&song.artist).color(p.secondary));
            ui.add_space(6.);
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "{} full {} · {} listened",
                    song.completed,
                    if song.completed == 1 {
                        "listen"
                    } else {
                        "listens"
                    },
                    collection_gallery::elapsed(song.listen_ms)
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("Drag to rotate")
                            .font(theme::regular(11.))
                            .color(p.secondary),
                    );
                });
            });
        });
    close || modal.should_close()
}
