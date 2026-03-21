#![allow(dead_code, unused_imports)]

mod desktop;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

fn main() {
    psh_core::logging::init("psh_launch");
    tracing::info!("starting psh-launch");

    let cfg = psh_core::config::load().expect("failed to load config");
    let max_results = cfg.launch.max_results.unwrap_or(20);

    let app = gtk4::Application::builder()
        .application_id("com.psh.launch")
        .build();

    app.connect_activate(move |app| {
        psh_core::theme::apply_theme(&cfg.theme.name);

        let entries = desktop::load_desktop_entries();
        tracing::info!("loaded {} desktop entries", entries.len());

        let window = gtk4::ApplicationWindow::builder()
            .application(app)
            .default_width(600)
            .default_height(400)
            .decorated(false)
            .build();

        window.init_layer_shell();
        window.set_layer(gtk4_layer_shell::Layer::Overlay);
        window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::Exclusive);
        window.set_anchor(gtk4_layer_shell::Edge::Top, true);
        window.set_margin(gtk4_layer_shell::Edge::Top, 200);

        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.add_css_class("psh-launch");

        let search_entry = gtk4::SearchEntry::new();
        search_entry.add_css_class("psh-launch-search");
        search_entry.set_hexpand(true);
        container.append(&search_entry);

        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_min_content_height(300);

        let list_box = gtk4::ListBox::new();
        list_box.add_css_class("psh-launch-results");
        list_box.set_selection_mode(gtk4::SelectionMode::Single);
        scroll.set_child(Some(&list_box));
        container.append(&scroll);

        // Populate initial list
        let entries_clone = entries.clone();
        for entry in entries_clone.iter().take(max_results) {
            let row = create_entry_row(entry);
            list_box.append(&row);
        }

        // Search filtering
        let entries_search = entries.clone();
        let list_box_clone = list_box.clone();
        search_entry.connect_search_changed(move |entry| {
            let query = entry.text().to_string();
            while let Some(row) = list_box_clone.row_at_index(0) {
                list_box_clone.remove(&row);
            }

            if query.is_empty() {
                for e in entries_search.iter().take(max_results) {
                    list_box_clone.append(&create_entry_row(e));
                }
                return;
            }

            let mut matcher = Matcher::new(Config::DEFAULT);
            let pattern =
                Pattern::new(&query, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy);

            let mut scored: Vec<_> = entries_search
                .iter()
                .filter_map(|e| {
                    let mut buf = Vec::new();
                    let haystack = nucleo_matcher::Utf32Str::new(&e.name, &mut buf);
                    pattern.score(haystack, &mut matcher).map(|s| (e, s))
                })
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));

            for (e, _) in scored.into_iter().take(max_results) {
                list_box_clone.append(&create_entry_row(e));
            }
        });

        // Launch on row activation
        let window_clone = window.clone();
        list_box.connect_row_activated(move |_, row| {
            if let Some(entry) = row
                .widget_name()
                .strip_prefix("entry:")
                .and_then(|name| entries.iter().find(|e| e.exec == name))
            {
                tracing::info!("launching: {}", entry.exec);
                let _ = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&entry.exec)
                    .spawn();
            }
            window_clone.close();
        });

        // Close on Escape
        let key_controller = gtk4::EventControllerKey::new();
        let window_esc = window.clone();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk4::gdk::Key::Escape {
                window_esc.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(key_controller);

        window.set_child(Some(&container));
        window.present();
        search_entry.grab_focus();
    });

    app.run_with_args::<String>(&[]);
}

fn create_entry_row(entry: &desktop::DesktopEntry) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.set_widget_name(&format!("entry:{}", entry.exec));

    let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    hbox.set_margin_top(8);
    hbox.set_margin_bottom(8);
    hbox.set_margin_start(12);
    hbox.set_margin_end(12);

    let label = gtk4::Label::new(Some(&entry.name));
    label.set_halign(gtk4::Align::Start);
    hbox.append(&label);

    if let Some(ref comment) = entry.comment {
        let desc = gtk4::Label::new(Some(comment));
        desc.add_css_class("dim-label");
        desc.set_halign(gtk4::Align::Start);
        desc.set_hexpand(true);
        hbox.append(&desc);
    }

    row.set_child(Some(&hbox));
    row
}
