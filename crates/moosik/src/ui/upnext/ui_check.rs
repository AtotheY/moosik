//! Offscreen interaction check for the queue's upcoming list.
use super::{empty_state, upcoming_rows};
use crate::{
    app::RepeatMode, images::Images, listening::Metadata, queue::PlayQueue, stats::Stats, theme, ui,
};
use egui_kittest::{Harness, kittest::Queryable};
use std::{path::Path, sync::Arc};

struct OfflineArtwork;

impl ui::FetchArtwork for OfflineArtwork {
    fn fetch_image(&self, _: Arc<str>, _: String, _: u32, _: bool) {
        panic!("queue fixtures must use local artwork");
    }
}

fn tracks(titles: &[&str]) -> Vec<crate::model::Track> {
    titles
        .iter()
        .enumerate()
        .map(|(i, title)| {
            Metadata {
                id: format!("fixture-{i}"),
                title: (*title).to_owned(),
                artists: vec![("Studio Ensemble".to_owned(), None)],
                duration: 181.0 + i as f64,
                ..Default::default()
            }
            .to_track()
        })
        .collect()
}

fn screenshot(h: &mut Harness<'_, PlayQueue>, dir: &Path, name: &str) {
    let path = dir.join(format!("{name}.png"));
    h.render()
        .expect("render offscreen queue")
        .save(&path)
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

#[test]
#[ignore = "run scripts/ui-check queue to capture rendered artifacts"]
fn queue() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("MOOSIK_UI_CHECK_DIR").expect("output directory"),
    );
    for (size, width) in [("normal", 800.0), ("narrow", 320.0)] {
        let mut images = Images::new();
        let mut stats = Stats::new(false);
        let palette = theme::Palette::dark();
        let mut initialized = false;
        let mut h = Harness::builder().with_size([width, 360.0]).build_ui_state(
            |ui, queue: &mut PlayQueue| {
                if !initialized {
                    theme::install(ui.ctx());
                    theme::apply(ui.ctx(), &palette);
                    initialized = true;
                    return; // Named fonts take effect on the next frame.
                }
                egui::Frame::new()
                    .fill(palette.card_fill())
                    .inner_margin(theme::PAGE_INSET_X as i8)
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        ui::page_title(ui, &palette, "Queue");
                        let upcoming = queue.upcoming(RepeatMode::Off, None);
                        if upcoming.is_empty() {
                            empty_state(ui, &palette);
                            return;
                        }
                        ui::queue_edit::prepare(
                            crate::ui::queue_edit::Scope::Personal,
                            ui,
                            queue.edit_stamp(),
                            &upcoming,
                        );
                        ui::queue_edit::toolbar(
                            crate::ui::queue_edit::Scope::Personal,
                            ui,
                            &palette,
                            "Next up",
                            false,
                            |_| {},
                        );
                        ui.add_space(4.0);
                        let now = Default::default();
                        let result = ui::spec_rows(
                            ui,
                            &palette,
                            &mut images,
                            &OfflineArtwork,
                            &mut stats,
                            &Default::default(),
                            upcoming.len(),
                            upcoming_rows(queue, &upcoming, &now, None, &[], false),
                        );
                        if let Some(action) = result.action {
                            let ui::RowAction::RemoveFromQueue(index) = action else {
                                panic!("clicking Remove from queue emitted a different row action");
                            };
                            assert!(result.click.is_none(), "removing also activated the row");
                            assert!(queue.remove(index));
                        }
                    });
            },
            PlayQueue::new(7),
        );
        h.step();
        h.get_by_label("Queue is empty");
        screenshot(&mut h, &dir, &format!("{size}-empty"));

        // Use a current track so list positions and underlying queue indices differ.
        h.state_mut().replace(
            tracks(&["Opening", "Northbound", "Interlude", "Night Train"]),
            None,
            0,
            None,
        );
        h.state_mut().set_current(0);
        h.step();
        assert_eq!(h.query_all_by_label("Remove from queue").count(), 3);
        let remove = h.get_all_by_label("Remove from queue").nth(1).unwrap();
        remove.hover();
        h.step();
        screenshot(&mut h, &dir, &format!("{size}-populated"));

        h.get_all_by_label("Remove from queue")
            .nth(1)
            .unwrap()
            .click();
        h.step();
        assert_eq!(
            h.state()
                .tracks()
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            ["Opening", "Northbound", "Night Train"]
        );
        assert_eq!(h.state().current(), Some(0));
        h.remove_cursor();
        h.step();
        assert_eq!(h.query_all_by_label("Remove from queue").count(), 2);

        h.get_all_by_label("Remove from queue")
            .next()
            .unwrap()
            .click();
        h.step();
        h.remove_cursor();
        h.step();
        assert_eq!(h.state().upcoming(RepeatMode::Off, None), [1]);
        assert_eq!(h.state().tracks()[1].title, "Night Train");
        assert_eq!(h.query_all_by_label("Remove from queue").count(), 1);
        screenshot(&mut h, &dir, &format!("{size}-sparse"));
    }
}
