use gtk4::glib;
use gtk4::prelude::*;

pub fn widget() -> gtk4::Widget {
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

fn update_time(label: &gtk4::Label) {
    let now = glib::DateTime::now_local().unwrap();
    let text = now.format("%H:%M:%S").unwrap();
    label.set_text(&text);
}
