//! Battery module — displays charge level and status from sysfs.

use std::fs;
use std::path::Path;

use gtk4::glib;
use gtk4::prelude::*;

use super::{BarModule, ModuleContext};

/// Displays battery percentage and charging status.
///
/// Reads from `/sys/class/power_supply/{device}/capacity` and `status`
/// every 30 seconds. Falls back to "AC" if no battery is found.
pub struct BatteryModule;

impl BarModule for BatteryModule {
    fn name(&self) -> &'static str {
        "battery"
    }

    fn widget(&self, ctx: &ModuleContext) -> gtk4::Widget {
        let device = ctx
            .config
            .battery_device
            .clone()
            .unwrap_or_else(|| "BAT0".into());

        let label = gtk4::Label::new(None);
        label.add_css_class("psh-bar-battery");

        update_battery(&label, &device);

        let label_clone = label.clone();
        let device_clone = device.clone();
        glib::timeout_add_seconds_local(30, move || {
            update_battery(&label_clone, &device_clone);
            glib::ControlFlow::Continue
        });

        label.upcast()
    }
}

/// Update the label with current battery capacity and status.
fn update_battery(label: &gtk4::Label, device: &str) {
    let (icon, text) = read_battery(device);
    label.set_text(&text);

    // Toggle low-battery CSS class
    label.remove_css_class("low");
    if icon == "LOW" {
        label.add_css_class("low");
    }
}

/// Read battery info from sysfs. Returns `(icon, display_text)`.
pub(crate) fn read_battery(device: &str) -> (&'static str, String) {
    let bat_path = Path::new("/sys/class/power_supply").join(device);
    if !bat_path.exists() {
        return ("AC", "AC".into());
    }

    let capacity = parse_capacity(&bat_path);
    let status = read_status(&bat_path);

    let icon = battery_icon(&status, capacity);
    (icon, format!("{icon} {capacity}%"))
}

/// Parse the battery capacity from sysfs.
fn parse_capacity(bat_path: &Path) -> u32 {
    fs::read_to_string(bat_path.join("capacity"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

/// Read the battery status string from sysfs.
fn read_status(bat_path: &Path) -> String {
    fs::read_to_string(bat_path.join("status"))
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Choose the display icon based on battery state.
fn battery_icon(status: &str, capacity: u32) -> &'static str {
    if status == "Charging" {
        "CHG"
    } else if capacity > 20 {
        "BAT"
    } else {
        "LOW"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_icon_charging() {
        assert_eq!(battery_icon("Charging", 50), "CHG");
        assert_eq!(battery_icon("Charging", 5), "CHG");
    }

    #[test]
    fn battery_icon_normal() {
        assert_eq!(battery_icon("Discharging", 80), "BAT");
        assert_eq!(battery_icon("Discharging", 21), "BAT");
    }

    #[test]
    fn battery_icon_low() {
        assert_eq!(battery_icon("Discharging", 20), "LOW");
        assert_eq!(battery_icon("Discharging", 5), "LOW");
        assert_eq!(battery_icon("Discharging", 0), "LOW");
    }

    #[test]
    fn read_battery_no_device() {
        // A device that doesn't exist should return AC
        let (icon, text) = read_battery("NONEXISTENT_BAT_999");
        assert_eq!(icon, "AC");
        assert_eq!(text, "AC");
    }
}
