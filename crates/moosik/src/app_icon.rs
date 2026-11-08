//! Bundled icon finishes. Settings store only a stable enum, never an arbitrary file path.
#[cfg(target_os = "windows")]
mod taskbar;
#[cfg(target_os = "windows")]
pub use taskbar::init_process_identity;

use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppIcon {
    Celadon,
    Midnight,
    Obsidian,
    Amber,
    Frost,
    Travertine,
    Copper,
    Ink,
    Bloom,
    Pearl,
    Vinyl,
    Amethyst,
    Matcha,
    Solar,
    Denim,
    Marble,
    Lagoon,
    Cherry,
    Carbon,
    #[default]
    #[serde(other)]
    Ceramic,
}
static CURRENT: AtomicUsize = AtomicUsize::new(0);
impl AppIcon {
    pub const ALL: [Self; 20] = [
        Self::Ceramic,
        Self::Celadon,
        Self::Midnight,
        Self::Obsidian,
        Self::Amber,
        Self::Frost,
        Self::Travertine,
        Self::Copper,
        Self::Ink,
        Self::Bloom,
        Self::Pearl,
        Self::Vinyl,
        Self::Amethyst,
        Self::Matcha,
        Self::Solar,
        Self::Denim,
        Self::Marble,
        Self::Lagoon,
        Self::Cherry,
        Self::Carbon,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Ceramic => "Ceramic",
            Self::Celadon => "Celadon",
            Self::Midnight => "Midnight",
            Self::Obsidian => "Obsidian",
            Self::Amber => "Amber",
            Self::Frost => "Frost",
            Self::Travertine => "Travertine",
            Self::Copper => "Copper",
            Self::Ink => "Ink",
            Self::Bloom => "Bloom",
            Self::Pearl => "Pearl",
            Self::Vinyl => "Vinyl",
            Self::Amethyst => "Amethyst",
            Self::Matcha => "Matcha",
            Self::Solar => "Solar",
            Self::Denim => "Denim",
            Self::Marble => "Marble",
            Self::Lagoon => "Lagoon",
            Self::Cherry => "Cherry",
            Self::Carbon => "Carbon",
        }
    }
    pub fn png(self) -> &'static [u8] {
        match self {
            Self::Ceramic => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/ceramic.png")
            }
            Self::Celadon => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/celadon.png")
            }
            Self::Midnight => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/midnight.png")
            }
            Self::Obsidian => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/obsidian.png")
            }
            Self::Amber => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/amber.png")
            }
            Self::Frost => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/frost.png")
            }
            Self::Travertine => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/travertine.png")
            }
            Self::Copper => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/copper.png")
            }
            Self::Ink => include_bytes!("../../../assets/branding/moosik/eclipse/variants/ink.png"),
            Self::Bloom => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/bloom.png")
            }
            Self::Pearl => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/pearl.png")
            }
            Self::Vinyl => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/vinyl.png")
            }
            Self::Amethyst => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/amethyst.png")
            }
            Self::Matcha => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/matcha.png")
            }
            Self::Solar => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/solar.png")
            }
            Self::Denim => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/denim.png")
            }
            Self::Marble => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/marble.png")
            }
            Self::Lagoon => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/lagoon.png")
            }
            Self::Cherry => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/cherry.png")
            }
            Self::Carbon => {
                include_bytes!("../../../assets/branding/moosik/eclipse/variants/carbon.png")
            }
        }
    }
    pub fn current() -> Self {
        Self::ALL[CURRENT.load(Ordering::Relaxed)]
    }
    pub fn image(self) -> anyhow::Result<image::RgbaImage> {
        Ok(image::load_from_memory(self.png())?.into_rgba8())
    }
    pub fn window_data(self) -> anyhow::Result<Arc<egui::IconData>> {
        let image = self.image()?;
        let (width, height) = image.dimensions();
        Ok(Arc::new(egui::IconData {
            width,
            height,
            rgba: image.into_raw(),
        }))
    }
    pub fn texture(self, ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let id = egui::Id::new(("moosik-icon", self.label()));
        if let Some(texture) = ctx.data_mut(|d| d.get_temp::<egui::TextureHandle>(id)) {
            return Some(texture);
        }
        let img = self.image().ok()?;
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [img.width() as usize, img.height() as usize],
            img.as_raw(),
        );
        let texture = ctx.load_texture(
            format!("moosik-icon-{}", self.label()),
            color,
            egui::TextureOptions::LINEAR,
        );
        ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
        Some(texture)
    }
    /// Called on the UI thread at launch and when the user chooses a finish.
    pub fn apply(
        self,
        #[cfg(not(target_os = "macos"))] ctx: &egui::Context,
        #[cfg(not(target_os = "macos"))] initial_icon: Option<Arc<egui::IconData>>,
        #[cfg(target_os = "windows")] window: Option<&winit::window::Window>,
    ) -> anyhow::Result<()> {
        #[cfg(not(target_os = "macos"))]
        let update_viewport = initial_icon.is_none();
        #[cfg(not(target_os = "macos"))]
        let icon = match initial_icon {
            Some(icon) => icon,
            None => self.window_data()?,
        };
        #[cfg(target_os = "macos")]
        {
            use objc2::{AnyThread, MainThreadMarker};
            use objc2_app_kit::{NSApplication, NSImage};
            use objc2_foundation::NSData;
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| anyhow::anyhow!("icon update needs the main thread"))?;
            let data = NSData::with_bytes(self.png());
            let icon = NSImage::initWithData(NSImage::alloc(), &data)
                .ok_or_else(|| anyhow::anyhow!("could not decode the Dock icon"))?;
            // SAFETY: main thread; the NSImage is valid and AppKit retains the assigned image.
            unsafe {
                NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&icon));
            }
        }
        #[cfg(target_os = "windows")]
        if let Some(window) = window {
            use winit::platform::windows::WindowExtWindows;
            let img = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(
                icon.width,
                icon.height,
                icon.rgba.as_slice(),
            )
            .ok_or_else(|| anyhow::anyhow!("invalid window icon dimensions"))?;
            let large =
                image::imageops::resize(&img, 64, 64, image::imageops::FilterType::Lanczos3);
            window.set_taskbar_icon(Some(winit::window::Icon::from_rgba(
                large.into_raw(),
                64,
                64,
            )?));
            if let Err(error) = self.set_taskbar_group(window) {
                tracing::warn!(%error, "could not apply the taskbar group icon");
            }
            self.refresh_pin();
        }
        #[cfg(not(target_os = "macos"))]
        if update_viewport {
            ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(icon)));
        }
        CURRENT.store(
            Self::ALL.iter().position(|v| *v == self).unwrap_or(0),
            Ordering::Relaxed,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_and_future_settings_keep_other_preferences() {
        for value in [
            serde_json::json!({"blur":false}),
            serde_json::json!({"app_icon":"future_finish", "blur":false}),
        ] {
            let settings: crate::settings::Settings =
                serde_json::from_value(value.clone()).unwrap();
            assert_eq!(settings.app_icon, AppIcon::Ceramic);
            if value.get("blur").is_some() {
                assert!(!settings.blur);
            }
        }
    }
    #[test]
    fn all_selections_round_trip_and_decode_as_distinct_icons() {
        let mut images = std::collections::HashSet::new();
        for icon in AppIcon::ALL {
            let settings: crate::settings::Settings =
                serde_json::from_value(serde_json::json!({"app_icon":icon, "blur":false})).unwrap();
            let bytes = serde_json::to_vec(&settings).unwrap();
            let restored: crate::settings::Settings = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(restored.app_icon, icon);
            assert!(!restored.blur);
            let img = icon.image().unwrap();
            assert_eq!(img.dimensions(), (512, 512));
            assert_eq!(img.get_pixel(0, 0).0[3], 0);
            assert!(images.insert(img.into_raw()), "duplicate icon");
        }
    }
}
