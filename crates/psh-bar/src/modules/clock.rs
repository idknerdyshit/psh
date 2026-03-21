//! Clock module — displays a live-updating time label.

use gtk4::glib;
use gtk4::prelude::*;

use super::{BarModule, ModuleContext};

/// Displays the current time, updated every second.
pub struct ClockModule;

impl BarModule for ClockModule {
    fn name(&self) -> &'static str {
        "clock"
    }

    fn widget(&self, _ctx: &ModuleContext) -> gtk4::Widget {
        let label = gtk4::Label::new(None);
        label.add_css_class("psh-bar-clock");

        update_time(&label);

        let label_clone = label.clone();
        glib::timeout_add_seconds_local(1, move || {
            update_time(&label_clone);
            glib::ControlFlow::Continue
        });

        label.upcast()
    }
}

/// Update the label text with the current local time.
///
/// Gracefully handles `now_local()` or `format()` failing (e.g. misconfigured
/// timezone) by leaving the label unchanged rather than panicking.
fn update_time(label: &gtk4::Label) {
    let Ok(now) = glib::DateTime::now_local() else {
        tracing::warn!("failed to get local time");
        return;
    };
    if let Ok(text) = now.format("%H:%M:%S") {
        label.set_text(&text);
    }
}
