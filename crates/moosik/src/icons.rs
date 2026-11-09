//! Lucide icons (ISC), registered once as `bytes://` sources and tinted per use.
//!
//! Generated from `assets/icons/*.svg`. egui_extras' SVG loader rasterises at the requested size,
//! so an icon stays crisp at any scale.

use egui::Color32;

macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
        pub enum Icon { $($variant),* }

        const ALL: &[(Icon, &str, &[u8])] = &[$((
            Icon::$variant,
            concat!("bytes://ytm-icon-", $file, ".svg"),
            include_bytes!(concat!("../../../assets/icons/", $file, ".svg")).as_slice(),
        )),*];
    };
}

icons! {
    ArrowUpDown => "arrow-up-down",
    ChevronLeft => "chevron-left",
    ChevronDown => "chevron-down",
    ChevronRight => "chevron-right",
    Check => "check",
    Compass => "compass",
    CircleUser => "circle-user",
    Bookmark => "bookmark",
    CircleAlert => "circle-alert",
    Disc3 => "disc-3",
    Download => "download",
    Ellipsis => "ellipsis",
    EllipsisVertical => "ellipsis-vertical",
    Folder => "folder",
    Grip => "grip",
    Heart => "heart",
    HeartFilled => "heart-filled",
    History => "history",
    House => "house",
    Info => "info",
    Library => "library",
    Link => "link",
    ListPlus => "list-plus",
    ListStart => "list-start",
    ListX => "list-x",
    ListMusic => "list-music",
    LoaderCircle => "loader-circle",
    LogOut => "log-out",
    MicVocal => "mic-vocal",
    Maximize2 => "maximize-2",
    Minimize2 => "minimize-2",
    Minus => "minus",
    Music => "music",
    PanelLeft => "panel-left",
    PauseFilled => "pause-filled",
    Pencil => "pencil",
    PlayFilled => "play-filled",
    Plus => "plus",
    Radio => "radio",
    RefreshCw => "refresh-cw",
    Repeat => "repeat",
    Repeat1 => "repeat-1",
    Search => "search",
    Settings => "settings",
    Shuffle => "shuffle",
    Smile => "smile",
    SkipBackFilled => "skip-back-filled",
    SkipForwardFilled => "skip-forward-filled",
    Square => "square",
    ThumbsDown => "thumbs-down",
    ThumbsDownFilled => "thumbs-down-filled",
    ThumbsUp => "thumbs-up",
    ThumbsUpFilled => "thumbs-up-filled",
    Trophy => "trophy",
    Trash2 => "trash-2",
    TrendingDown => "trending-down",
    TrendingUp => "trending-up",
    Video => "video",
    Volume1 => "volume-1",
    Volume2 => "volume-2",
    VolumeX => "volume-x",
    WifiOff => "wifi-off",
    X => "x",
}

impl Icon {
    pub fn uri(self) -> &'static str {
        ALL.iter()
            .find(|(i, _, _)| *i == self)
            .map(|(_, u, _)| *u)
            .unwrap()
    }

    /// An `Image` sized and tinted for immediate use.
    pub fn image(self, color: Color32, size: f32) -> egui::Image<'static> {
        egui::Image::new(self.uri())
            .fit_to_exact_size(egui::vec2(size, size))
            .tint(color)
    }
}

pub fn paint_source(ui: &egui::Ui, rect: egui::Rect, source: &str, color: Color32) {
    let point = |x: f32, y: f32| rect.min + egui::vec2(x, y) * (rect.width() / 24.0);
    let stroke = egui::Stroke::new(rect.width() / 14.0, color);
    match source {
        "Spotify" => {
            ui.painter()
                .circle_stroke(rect.center(), rect.width() * 0.45, stroke);
            for (left, right, y, bend) in [
                (5.5, 18.5, 9.0, 3.8),
                (6.5, 17.5, 12.5, 3.0),
                (7.5, 16.5, 16.0, 2.2),
            ] {
                let points = (0..=12)
                    .map(|i| {
                        let t = i as f32 / 12.0;
                        point(left + (right - left) * t, y - bend * 4.0 * t * (1.0 - t))
                    })
                    .collect();
                ui.painter().add(egui::Shape::line(points, stroke));
            }
        }
        "YouTube Music" => {
            ui.painter().rect_stroke(
                egui::Rect::from_min_max(point(1.5, 4.5), point(22.5, 19.5)),
                egui::CornerRadius::same((rect.width() / 8.0).round() as u8),
                stroke,
                egui::StrokeKind::Inside,
            );
            ui.painter().add(egui::Shape::convex_polygon(
                vec![point(9.5, 8.0), point(16.0, 12.0), point(9.5, 16.0)],
                color,
                egui::Stroke::NONE,
            ));
        }
        _ => Icon::Music.image(color, rect.width()).paint_at(ui, rect),
    }
}

/// Register every icon as an in-memory byte source. Call once at startup.
pub fn register(ctx: &egui::Context) {
    for (_, uri, bytes) in ALL {
        ctx.include_bytes(*uri, *bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_has_a_unique_uri() {
        let mut uris: Vec<&str> = ALL.iter().map(|(_, u, _)| *u).collect();
        let total = uris.len();
        uris.sort_unstable();
        uris.dedup();
        assert_eq!(uris.len(), total, "duplicate icon uri");
    }

    #[test]
    fn every_icon_carries_svg_bytes() {
        for (icon, _, bytes) in ALL {
            assert!(!bytes.is_empty(), "{icon:?} is empty");
            assert!(
                bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml"),
                "{icon:?} does not look like an SVG"
            );
        }
    }
}
