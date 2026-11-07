//! winit maps both a new finger gesture and the start of momentum to TouchPhase::Start.
//! Keep AppKit's finger-down timestamp so shelves can distinguish the two.
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventPhase};

pub(super) fn id() -> egui::Id {
    egui::Id::new("macos_scroll_gesture")
}

pub(crate) struct Monitor(Retained<AnyObject>);

impl Monitor {
    pub(crate) fn install(ctx: egui::Context) -> Option<Self> {
        let handler = RcBlock::new(move |ptr: NonNull<NSEvent>| {
            // AppKit passes a live event for the duration of the callback.
            let event = unsafe { ptr.as_ref() };
            if event.phase() == NSEventPhase::Began {
                ctx.data_mut(|d| d.insert_temp(id(), event.timestamp().to_bits()));
            } else if event.phase().is_empty() && event.momentumPhase().is_empty() {
                // A conventional scroll wheel has no gesture phases.
                ctx.data_mut(|d| d.remove::<u64>(id()));
            }
            ptr.as_ptr()
        });
        // The handler always returns its original event to AppKit.
        unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::ScrollWheel,
                &handler,
            )
        }
        .map(Self)
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        // This is the token returned by addLocalMonitor above.
        unsafe { NSEvent::removeMonitor(&self.0) };
    }
}
