//! A local genre atlas of the same tagged listening used by the Genres list.
use super::{App, Report, Tab, art, clock, select_song, small, theme, widgets};
use crate::listening::{Row, collection::artist_key};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Weak};

#[derive(Default)]
struct Artist {
    key: String,
    name: String,
    time: f64,
    tracks: Vec<usize>,
}
#[derive(Default)]
struct Region {
    key: String,
    name: String,
    time: f64,
    artists: Vec<Artist>,
    color: usize,
}
#[derive(Default)]
pub(super) struct View {
    report: Weak<Report>,
    colors: BTreeMap<String, usize>,
    regions: Vec<Region>,
    tagged: f64,
    genre: String,
    artist: String,
}
fn regions(tracks: &[Row]) -> (Vec<Region>, f64) {
    let mut groups = BTreeMap::new();
    let mut tagged = 0.;
    for (i, row) in tracks.iter().enumerate().filter(|(_, r)| r.listen_ms > 0.) {
        let mut seen = HashSet::new();
        for label in row
            .genres
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let key = label.to_lowercase();
            if !seen.insert(key.clone()) {
                continue;
            }
            let (region, artists) = groups.entry(key.clone()).or_insert_with(|| {
                (
                    Region {
                        key,
                        name: label.to_owned(),
                        ..Default::default()
                    },
                    BTreeMap::new(),
                )
            });
            region.time += row.listen_ms;
            let mut credited = HashSet::new();
            for (name, id) in row.metadata.iter().flat_map(|m| &m.artists) {
                let key = artist_key(name, id.as_deref());
                if name.trim().is_empty() || !credited.insert(key.clone()) {
                    continue;
                }
                let artist = artists.entry(key.clone()).or_insert_with(|| Artist {
                    key,
                    name: name.clone(),
                    ..Default::default()
                });
                artist.time += row.listen_ms;
                artist.tracks.push(i);
            }
        }
        if !seen.is_empty() {
            tagged += row.listen_ms;
        }
    }
    let mut regions: Vec<_> = groups
        .into_values()
        .map(|(mut region, artists)| {
            region.artists = artists.into_values().collect();
            region
                .artists
                .sort_by(|a, b| b.time.total_cmp(&a.time).then(a.key.cmp(&b.key)));
            for a in &mut region.artists {
                a.tracks.sort_by(|&i, &j| {
                    tracks[j]
                        .plays
                        .cmp(&tracks[i].plays)
                        .then(tracks[j].listen_ms.total_cmp(&tracks[i].listen_ms))
                        .then(tracks[i].id.cmp(&tracks[j].id))
                });
            }
            region
        })
        .collect();
    regions.sort_by(|a, b| b.time.total_cmp(&a.time).then(a.key.cmp(&b.key)));
    (regions, tagged)
}

const TINTS: [Color32; 6] = [
    Color32::from_rgb(192, 137, 184),
    Color32::from_rgb(112, 179, 161),
    Color32::from_rgb(130, 165, 208),
    Color32::from_rgb(220, 159, 116),
    Color32::from_rgb(158, 148, 211),
    Color32::from_rgb(182, 177, 117),
];
// Generate nested, closed contour lines from one shared coastline per region.
// The fill uses a triangle fan so concave bends stay inside the contour.
fn island(ui: &Ui, r: Rect, tint: Color32, selected: bool, seed: f32, p: &theme::Palette) {
    let paint = ui.painter();
    let center = r.center();
    let coast: Vec<_> = (0..84)
        .map(|i| {
            let a = i as f32 * std::f32::consts::TAU / 84.;
            let ripple = 0.91
                + 0.074 * (3. * a + seed).sin()
                + 0.045 * (5. * a - seed).cos()
                + 0.014 * (8. * a + seed).sin();
            vec2(a.cos() * r.width(), a.sin() * r.height()) * (ripple * 0.50)
        })
        .collect();
    let fill = widgets::mix(p.panel, tint, if selected { 0.19 } else { 0.112 });
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, fill);
    for offset in &coast {
        mesh.colored_vertex(center + *offset, fill);
    }
    for i in 0..coast.len() {
        mesh.add_triangle(0, i as u32 + 1, ((i + 1) % coast.len()) as u32 + 1);
    }
    paint.add(mesh);
    for (n, scale) in [1., 0.94, 0.86, 0.76].into_iter().enumerate() {
        let line = coast
            .iter()
            .map(|v| center + *v * scale + vec2(1.6 * n as f32, -0.8 * n as f32))
            .collect();
        paint.add(egui::Shape::closed_line(
            line,
            Stroke::new(
                if selected && n == 0 { 1.35 } else { 0.85 },
                widgets::mix(
                    p.panel,
                    tint,
                    if n == 0 {
                        0.68
                    } else {
                        0.46 - n as f32 * 0.065
                    },
                ),
            ),
        ));
    }
}
fn chooser(
    ui: &mut Ui,
    p: &theme::Palette,
    id: &str,
    label: &str,
    items: impl Iterator<Item = (usize, String)>,
) -> Option<usize> {
    let current = widgets::truncate_to(
        ui,
        label,
        theme::medium(13.),
        (ui.available_width() - 46.).clamp(50., 220.),
    );
    let hit = widgets::outline_button(ui, p, &format!("{current}  ⌄"));
    let mut choice = None;
    egui::Popup::menu(&hit)
        .id(ui.id().with(id))
        .width(216_f32.min(ui.ctx().content_rect().width() - 24.))
        .frame(widgets::menu_frame(ui, p))
        .show(|ui| {
            ui.set_width(200_f32.min(ui.ctx().content_rect().width() - 40.));
            egui::ScrollArea::vertical()
                .max_height(260.)
                .show(ui, |ui| {
                    for (i, item) in items {
                        if widgets::menu_item_plain(ui, p, &item, None).clicked() {
                            choice = Some(i);
                            ui.close();
                        }
                    }
                });
        });
    choice
}

pub(super) fn show(app: &mut App, ui: &mut Ui, report: &Arc<Report>) {
    let mut view = std::mem::take(&mut app.listening_view.atlas);
    if view.regions.is_empty() {
        super::empty(
            ui,
            &app.palette,
            "Your genre map",
            "Add a genre to a song in your listening history to place it on the map.",
        );
    } else {
        view.map(app, ui, report);
    }
    if view.tagged + 1. < report.listen_ms {
        ui.horizontal_wrapped(|ui| {
            small(
                ui,
                &app.palette,
                format!(
                    "{} of {} tagged",
                    clock(view.tagged),
                    clock(report.listen_ms)
                ),
            );
            if widgets::outline_button(ui, &app.palette, "Tag songs").clicked() {
                app.listening_view.group =
                    Some((Tab::Genres, "Unknown".into(), "Untagged songs".into()));
                app.listening_view.tab = Tab::Songs;
                app.listening_view.search.clear();
            }
        });
    }
    app.listening_view.atlas = view;
}
impl View {
    pub(super) fn filter(&mut self, ui: &mut Ui, p: &theme::Palette, report: &Arc<Report>) {
        if !self.report.ptr_eq(&Arc::downgrade(report)) {
            (self.regions, self.tagged) = regions(&report.tracks);
            for region in &mut self.regions {
                let next = self.colors.len() % TINTS.len();
                region.color = *self.colors.entry(region.key.clone()).or_insert(next);
            }
            self.report = Arc::downgrade(report);
        }
        if self.regions.len() > 3 {
            let selected = self
                .regions
                .iter()
                .find(|g| g.key == self.genre)
                .unwrap_or(&self.regions[0]);
            if let Some(i) = chooser(
                ui,
                p,
                "atlas-genre",
                &selected.name,
                self.regions
                    .iter()
                    .enumerate()
                    .map(|(i, g)| (i, g.name.clone())),
            ) {
                self.genre.clone_from(&self.regions[i].key);
                self.artist.clear();
            }
        }
    }
    fn map(&mut self, app: &mut App, ui: &mut Ui, r: &Report) {
        let p = app.palette;
        let selected = self
            .regions
            .iter()
            .position(|g| g.key == self.genre)
            .unwrap_or(0);
        self.genre.clone_from(&self.regions[selected].key);
        let group = &self.regions[selected];
        let mut artist = group
            .artists
            .iter()
            .position(|a| a.key == self.artist)
            .unwrap_or(0);
        let mut next = None;
        let width = ui.available_width();
        if width >= 820. && !group.artists.is_empty() {
            let detail_width = (width * 0.25).clamp(210., 288.);
            ui.horizontal_top(|ui| {
                let map_width = width - detail_width - ui.spacing().item_spacing.x;
                ui.allocate_ui_with_layout(
                    vec2(map_width, 0.),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(map_width);
                        next = self.graph(app, ui, r, selected, &mut artist);
                    },
                );
                ui.vertical(|ui| {
                    ui.set_width(detail_width);
                    ui.add_space(48.);
                    egui::Frame::new()
                        .fill(p.card_fill())
                        .inner_margin(10.)
                        .corner_radius(theme::RADIUS)
                        .show(ui, |ui| {
                            ui.set_width(detail_width - 20.);
                            artist_detail(app, ui, r, group, &mut artist);
                        });
                });
            });
        } else {
            next = self.graph(app, ui, r, selected, &mut artist);
            artist_detail(app, ui, r, group, &mut artist);
        }
        if let Some(next) = next {
            self.genre.clone_from(&self.regions[next].key);
            self.artist.clear();
        } else if let Some(a) = group.artists.get(artist) {
            self.artist.clone_from(&a.key);
        }
    }
    fn graph(
        &self,
        app: &mut App,
        ui: &mut Ui,
        r: &Report,
        selected: usize,
        active_artist: &mut usize,
    ) -> Option<usize> {
        let mut artist = *active_artist;
        let group = &self.regions[selected];
        let p = app.palette;
        let mut next = None;
        let start = selected / 3 * 3;
        let batch = &self.regions[start..(start + 3).min(self.regions.len())];
        let compact = ui.available_width() < 560.;
        let height = if compact {
            164. + (batch.len() - 1) as f32 * 86.
        } else {
            330.
        };
        let (canvas, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let times: Vec<_> = batch.iter().map(|g| g.time).collect();
        let slots = layout(canvas, &times, compact);
        let mut islands = Vec::new();
        for (j, (genre, slot)) in batch.iter().zip(slots).enumerate() {
            let region = slot.shrink(2.);
            islands.push(region);
            let on = j + start == selected;
            island(
                ui,
                region,
                TINTS[genre.color],
                on,
                (start + j) as f32 * 1.7 + 0.7,
                &p,
            );
            let hit = ui.interact(slot, ui.id().with(("region", &genre.key)), Sense::click());
            hit.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &genre.name)
            });
            if hit.clicked() && !on {
                next = Some(j + start);
                ui.ctx().request_repaint();
            }
            let y = if on && !genre.artists.is_empty() && region.height() > 122. {
                region.center().y - region.height() * 0.20
            } else {
                region.center().y - 7.
            };
            let namefont = theme::medium(if on {
                28.
            } else if compact {
                16.
            } else {
                22.
            });
            let label =
                widgets::truncate_to(ui, &genre.name, namefont.clone(), region.width() * 0.78);
            let mut title = egui::text::LayoutJob::default();
            title.append(
                &label,
                0.,
                egui::TextFormat {
                    font_id: namefont,
                    italics: true,
                    color: widgets::mix(p.text, TINTS[genre.color], 0.22),
                    ..Default::default()
                },
            );
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(title));
            ui.painter().galley(
                pos2(region.center().x, y) - galley.size() * 0.5,
                galley,
                p.text,
            );
            ui.painter().text(
                pos2(region.center().x, y + 20.),
                Align2::CENTER_CENTER,
                clock(genre.time),
                theme::regular(12.),
                p.secondary,
            );
        }
        // Pins are positioned inside the active region. Only draw a bridge for
        // a credited artist actually present in a second visible genre.
        let home = islands[selected - start];
        let limit = if home.width() > 230. && home.height() > 240. {
            3
        } else if home.height() > 122. {
            1
        } else {
            0
        };
        let mut pins: Vec<_> = (0..group.artists.len().min(limit)).collect();
        if !pins.is_empty() && !pins.contains(&artist) {
            *pins.last_mut().unwrap() = artist;
        }
        for (n, &ai) in pins.iter().enumerate() {
            let a = &group.artists[ai];
            let spread = home.width() > 265. && pins.len() > 1;
            let (dx, dy) = if spread {
                [(-0.14, 0.025), (0.17, 0.15), (-0.07, 0.285)][n]
            } else {
                (0., 0.08)
            };
            let center = home.center() + vec2(dx * home.width(), dy * home.height());
            let width = widgets::text_width(ui, &a.name, &theme::medium(12.)) + 46.;
            let slot = Rect::from_center_size(center, vec2(width.min(home.width() * 0.70), 32.));
            for (j, other) in batch
                .iter()
                .enumerate()
                .filter(|(j, _)| *j + start != selected)
            {
                if other.artists.iter().any(|b| b.key == a.key) {
                    let from = slot.right_center();
                    let to = islands[j].center() + vec2(0., islands[j].height() * 0.25);
                    let stroke = Stroke::new(1.15, widgets::mix(p.panel, TINTS[other.color], 0.75));
                    ui.painter()
                        .add(egui::epaint::CubicBezierShape::from_points_stroke(
                            [from, from + vec2(22., 8.), to + vec2(-24., 18.), to],
                            false,
                            Color32::TRANSPARENT,
                            stroke,
                        ));
                    ui.painter().circle_filled(to, 2.6, stroke.color);
                }
            }
            let hit = ui.interact(slot, ui.id().with(("pin", &a.key)), Sense::click());
            hit.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &a.name));
            if hit.clicked() {
                artist = ai;
            }
            if artist == ai || hit.hovered() {
                ui.painter()
                    .rect_filled(slot, 5., widgets::mix(p.panel, TINTS[group.color], 0.17));
            }
            let thumbnail = Rect::from_min_size(slot.min + vec2(3., 2.), vec2(28., 28.));
            art(app, ui, thumbnail, r.tracks[a.tracks[0]].artwork.as_deref());
            super::clipped(
                ui,
                &a.name,
                Rect::from_min_max(thumbnail.right_top() + vec2(5., 0.), slot.max),
                theme::medium(12.),
                p.text,
            );
        }
        *active_artist = artist;
        next
    }
}
fn artist_detail(
    app: &mut App,
    ui: &mut Ui,
    r: &Report,
    group: &Region,
    active_artist: &mut usize,
) {
    let p = app.palette;
    let mut artist = *active_artist;
    if let Some(a) = group.artists.get(artist) {
        ui.add_space(8.);
        if let Some(i) = chooser(
            ui,
            &p,
            "atlas-artist",
            &a.name,
            group
                .artists
                .iter()
                .enumerate()
                .map(|(i, a)| (i, a.name.clone())),
        ) {
            artist = i;
        }
        ui.add_space(5.);
        small(ui, &p, format!("Most-played tracks in {}", group.name));
        let a = &group.artists[artist];

        for &i in a.tracks.iter().take(3) {
            let row = &r.tracks[i];
            let (rect, hit) =
                ui.allocate_exact_size(vec2(ui.available_width(), 40.), Sense::click());
            super::row_hover(ui, &p, rect, &hit);
            let cover = Rect::from_min_size(rect.left_center() + vec2(5., -14.), vec2(28., 28.));
            art(app, ui, cover, row.artwork.as_deref());
            super::clipped(
                ui,
                &row.name,
                Rect::from_min_max(rect.min + vec2(44., 0.), rect.max - vec2(84., 0.)),
                theme::medium(13.),
                p.text,
            );
            super::value_at(ui, &p, rect, rect.right() - 6., &clock(row.listen_ms));
            hit.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &row.name)
            });
            if hit.clicked() {
                select_song(app, row);
            }
        }
    }
    *active_artist = artist;
}
// Reserve room for labels at the smallest sizes; distribute all remaining area by time.
// Selection never moves the regions, so the map stays legible while exploring.
fn layout(r: Rect, times: &[f64], compact: bool) -> Vec<Rect> {
    let total: f64 = times.iter().sum();
    let share = |i: usize| (times[i] / total) as f32;
    if times.len() == 1 {
        return vec![Rect::from_center_size(
            r.center(),
            vec2(r.width().min(620.), r.height()),
        )];
    }
    if compact {
        let mut y = r.top();
        let spare = r.height() - 68. * times.len() as f32;
        return (0..times.len())
            .map(|i| {
                let h = 68. + spare * share(i);
                let slot = Rect::from_min_size(pos2(r.left(), y), vec2(r.width(), h));
                y += h;
                slot
            })
            .collect();
    }
    let left_width = r.width() * share(0).clamp(0.36, 0.68);
    let left = Rect::from_min_size(r.min, vec2(left_width, r.height()));
    let right = Rect::from_min_max(left.right_top(), r.max);
    if times.len() == 2 {
        return vec![left, right];
    }
    let split =
        right.top() + right.height() * (times[1] / (times[1] + times[2])).clamp(0.35, 0.69) as f32;
    vec![
        left,
        Rect::from_min_max(right.min, pos2(right.right(), split)),
        Rect::from_min_max(pos2(right.left(), split), right.max),
    ]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_artists_and_tags_do_not_duplicate_time_or_tracks() {
        let make = |name: &str, genres: &str, time: f64, plays: u64| Row {
            id: name.into(),
            name: name.into(),
            genres: genres.into(),
            listen_ms: time,
            plays,
            metadata: Some(crate::listening::Metadata {
                artists: vec![
                    ("A".into(), Some("a".into())),
                    ("A".into(), Some("a".into())),
                ],
                ..Default::default()
            }),
            ..Default::default()
        };
        let tracks = [
            make("One", "Trip-hop, trip-hop, Electronic", 30., 1),
            make("Two", "Electronic", 20., 2),
            make("Three", "", 40., 3),
        ];
        let (groups, tagged) = regions(&tracks);
        assert_eq!(tagged, 50.);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].artists[0].tracks, [1, 0]);
        assert_eq!(groups[0].artists[0].time, 50.);
        assert_eq!(groups[1].time, 30.);
    }
}
