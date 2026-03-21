use std::fs;
use std::path::Path;

use gtk4::glib;
use gtk4::prelude::*;

pub fn widget() -> gtk4::Widget {
    let label = gtk4::Label::new(None);
    label.add_css_class("psh-bar-battery");

    update_battery(&label);

    let label_clone = label.clone();
    glib::timeout_add_seconds_local(30, move || {
        update_battery(&label_clone);
        glib::ControlFlow::Continue
    });

    label.upcast()
}

fn update_battery(label: &gtk4::Label) {
    let bat_path = Path::new("/sys/class/power_supply/BAT0");
    if !bat_path.exists() {
        label.set_text("AC");
        return;
    }

    let capacity = fs::read_to_string(bat_path.join("capacity"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    let status = fs::read_to_string(bat_path.join("status"))
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let icon = if status == "Charging" {
        "CHG"
    } else if capacity > 20 {
        "BAT"
    } else {
        "LOW"
    };

    label.set_text(&format!("{icon} {capacity}%"));
}
