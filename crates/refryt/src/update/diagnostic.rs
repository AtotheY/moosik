//! An isolated signed-bundle updater exercise. Never loads an account or listening profile.
use super::macos::{self, Event};
use crate::{theme, widgets};
use objc2_foundation::{NSBundle, NSString};
use std::path::PathBuf;

fn bundle_string(key: &str) -> Option<String> {
    NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str(key))?
        .downcast::<NSString>()
        .ok()
        .map(|s| s.to_string())
}
struct Probe {
    version: String,
    available: Option<String>,
    status: String,
    state_file: PathBuf,
    state: serde_json::Value,
}
impl eframe::App for Probe {
    fn logic(&mut self, _: &egui::Context, _: &mut eframe::Frame) {
        while let Some(event) = macos::poll() {
            match event {
                Event::Available(v) => {
                    self.status = "Update available".into();
                    self.available = Some(v);
                }
                Event::Current => self.status = "Up to date".into(),
                Event::Failed(error) => self.status = error,
                Event::Skipped => self.available = None,
                Event::Finished => {}
                Event::Relaunch => {
                    // Exercise the same checkpoint-before-relaunch contract with dummy data.
                    crate::persist::write_json_atomic(
                        &self.state_file,
                        &serde_json::to_vec(&self.state).unwrap(),
                    )
                    .unwrap();
                    macos::continue_relaunch();
                }
            }
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let p = theme::Palette::dark();
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.heading(format!("Moosik updater · {}", self.version));
            if let Some(version) = &self.available
                && crate::ui::update::indicator(ui, &p, version)
            {
                macos::check();
            }
        });
        ui.label(&self.status);
        ui.add_space(16.0);
        if widgets::outline_button(ui, &p, "Check for updates").clicked() {
            macos::check();
        }
        ui.add_space(24.0);
        ui.label("Isolated verification app · dummy queue and settings");
        ui.label(format!(
            "Queue: {} · position: {}",
            self.state["queue"], self.state["position"]
        ));
    }
}
/// Run only inside the separately signed updater-check fixture bundle.
pub fn run() -> eframe::Result {
    let state_file = PathBuf::from(
        bundle_string("MoosikUpdaterTestState").expect("use the updater fixture bundle"),
    );
    let version = bundle_string("CFBundleShortVersionString").unwrap();
    let state =
        serde_json::from_slice(&std::fs::read(&state_file).expect("dummy state fixture")).unwrap();
    eframe::run_native(
        "Moosik update verification",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([680.0, 360.0]),
            ..Default::default()
        },
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            macos::start(&cc.egui_ctx, true).expect("configured Sparkle fixture");
            Ok(Box::new(Probe {
                version,
                available: None,
                status: "Ready".into(),
                state_file,
                state,
            }))
        }),
    )
}
