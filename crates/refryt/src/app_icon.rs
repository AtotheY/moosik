//! Bundled icon finishes. Settings store only a stable enum, never an arbitrary file path.
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
    #[default]
    #[serde(other)]
    Ceramic,
}
static CURRENT: AtomicUsize = AtomicUsize::new(0);
impl AppIcon {
    pub const ALL: [Self; 10] = [
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
        }
    }
    pub fn current() -> Self {
        Self::ALL[CURRENT.load(Ordering::Relaxed)]
    }
    pub fn image(self) -> anyhow::Result<image::RgbaImage> {
        Ok(image::load_from_memory(self.png())?.to_rgba8())
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
    pub fn apply(self, ctx: &egui::Context) -> anyhow::Result<()> {
        let img = self.image()?;
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
        ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(Arc::new(
            egui::IconData {
                width: img.width(),
                height: img.height(),
                rgba: img.into_raw(),
            },
        ))));
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
    fn all_ten_selections_round_trip_and_decode_as_distinct_icons() {
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
