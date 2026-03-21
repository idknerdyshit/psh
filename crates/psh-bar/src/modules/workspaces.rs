use gtk4::prelude::*;

pub fn widget() -> gtk4::Widget {
    let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    container.add_css_class("psh-bar-workspaces");

    // Placeholder workspace buttons — will be replaced with niri IPC / ext-workspace
    for i in 1..=5 {
        let btn = gtk4::Button::with_label(&i.to_string());
        btn.add_css_class("psh-bar-workspace-btn");
        if i == 1 {
            btn.add_css_class("active");
        }
        container.append(&btn);
    }

    container.upcast()
}
