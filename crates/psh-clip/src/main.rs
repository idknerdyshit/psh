//! psh-clip — clipboard manager for the psh desktop environment.
//!
//! A long-lived daemon that monitors clipboard changes via `zwlr-data-control-v1`,
//! stores history persistently, and provides a GTK4 picker overlay for browsing
//! and pasting from clipboard history.

mod history;
mod monitor;
mod persist;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Instant;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;
use psh_core::config;
use tracing::{info, warn};

use history::{ClipEntry, ClipHistory};

fn main() {
    psh_core::logging::init("psh_clip");
    info!("starting psh-clip");

    let cfg = config::load().expect("failed to load config");
    let clip_cfg = cfg.clip.clone();

    let app = gtk4::Application::builder()
        .application_id("com.psh.clip")
        .build();

    app.connect_activate(move |app| {
        psh_core::theme::apply_theme(&cfg.theme.name);

        let persisted = if clip_cfg.persist {
            let entries = persist::load();
            if !entries.is_empty() {
                info!("loaded {} persisted clipboard entries", entries.len());
            }
            entries
        } else {
            Vec::new()
        };
        let history = ClipHistory::load_from(persisted, clip_cfg.max_history);

        persist::prune_orphaned_images(&history.items());

        // Channel for paste-on-select: GTK -> monitor thread
        let (set_tx, set_rx) = mpsc::channel::<ClipEntry>();

        // Channel for clipboard monitor -> GTK thread
        let (clip_tx, clip_rx) = async_channel::bounded::<ClipEntry>(64);

        let monitor_history = history.clone();
        let monitor_cfg = clip_cfg.clone();
        std::thread::spawn(move || {
            monitor::run_monitor(monitor_history, clip_tx, set_rx, monitor_cfg);
        });

        let persist_history = history.clone();
        let persist_enabled = clip_cfg.persist;
        glib::spawn_future_local(async move {
            let mut last_save = Instant::now();
            while let Ok(entry) = clip_rx.recv().await {
                persist_history.push(entry);
                // Debounced persistence — at most once per second
                if persist_enabled && last_save.elapsed().as_secs() >= 1 {
                    persist::save(&persist_history.items());
                    last_save = Instant::now();
                }
            }
            // Final save to ensure the last entry is persisted
            if persist_enabled {
                persist::save(&persist_history.items());
            }
        });

        // IPC listener for ShowClipboardHistory
        let (ipc_tx, ipc_rx) = async_channel::bounded::<()>(4);
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("failed to create tokio runtime for IPC: {e}");
                    return;
                }
            };
            rt.block_on(async {
                let Ok(mut stream) = psh_core::ipc::connect().await else {
                    warn!("could not connect to IPC hub, running standalone");
                    return;
                };
                loop {
                    match psh_core::ipc::recv(&mut stream).await {
                        Ok(psh_core::ipc::Message::ShowClipboardHistory) => {
                            let _ = ipc_tx.send(()).await;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!("ipc error: {e}");
                            break;
                        }
                    }
                }
            });
        });

        // Show picker when IPC message arrives
        let app_clone = app.clone();
        let picker_history = history.clone();
        glib::spawn_future_local(async move {
            while let Ok(()) = ipc_rx.recv().await {
                show_picker(&app_clone, &picker_history, &set_tx);
            }
        });
    });

    app.run_with_args::<String>(&[]);
}

/// Creates and shows the clipboard history picker overlay.
///
/// The picker is a layer-shell overlay with a search bar and a scrollable list
/// of history entries. Activating a row sends the entry over `set_tx` for the
/// monitor thread to set as the active clipboard, then closes the window.
/// Pressing Escape also closes the picker.
fn show_picker(
    app: &gtk4::Application,
    history: &ClipHistory,
    set_tx: &mpsc::Sender<ClipEntry>,
) {
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .default_width(500)
        .default_height(400)
        .decorated(false)
        .build();

    window.init_layer_shell();
    window.set_layer(gtk4_layer_shell::Layer::Overlay);
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::Exclusive);

    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    container.add_css_class("psh-clip");

    let title = gtk4::Label::new(Some("Clipboard History"));
    title.add_css_class("psh-clip-title");
    title.set_margin_top(12);
    title.set_margin_bottom(8);
    container.append(&title);

    let search_entry = gtk4::SearchEntry::new();
    search_entry.add_css_class("psh-clip-search");
    search_entry.set_margin_start(12);
    search_entry.set_margin_end(12);
    search_entry.set_margin_bottom(8);
    search_entry.set_placeholder_text(Some("Search clipboard..."));
    container.append(&search_entry);

    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_vexpand(true);

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("psh-clip-list");

    // Snapshot of items at picker open time — new entries won't appear until next open
    let all_items = Rc::new(RefCell::new(history.items()));

    populate_list(&list_box, &all_items.borrow());

    scroll.set_child(Some(&list_box));
    container.append(&scroll);

    let filter_list = list_box.clone();
    let filter_items = all_items.clone();
    let history_for_search = history.clone();
    search_entry.connect_search_changed(move |entry| {
        let query = entry.text();
        let filtered = if query.is_empty() {
            history_for_search.items()
        } else {
            history_for_search.search(&query)
        };
        *filter_items.borrow_mut() = filtered.clone();
        populate_list(&filter_list, &filtered);
    });

    let select_items = all_items.clone();
    let select_tx = set_tx.clone();
    let select_window = window.clone();
    list_box.connect_row_activated(move |_, row| {
        let idx = row.index() as usize;
        let items = select_items.borrow();
        if let Some(entry) = items.get(idx) {
            let _ = select_tx.send(entry.clone());
            info!("paste-on-select: setting clipboard to entry {idx}");
        }
        select_window.close();
    });

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

    // Focus the search entry after presenting
    search_entry.grab_focus();
}

/// Populates the list box with clip entries, clearing existing rows first.
///
/// Text entries are shown as truncated labels. Image entries show a 48px
/// thumbnail (if the cached file exists) alongside the MIME type and filename.
/// The first row is auto-selected after population.
fn populate_list(list_box: &gtk4::ListBox, items: &[ClipEntry]) {
    // Remove all existing rows
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    for item in items {
        let row = gtk4::ListBoxRow::new();
        match item {
            ClipEntry::Text { .. } => {
                let label = gtk4::Label::new(Some(&item.display_text()));
                label.set_halign(gtk4::Align::Start);
                label.set_margin_top(6);
                label.set_margin_bottom(6);
                label.set_margin_start(12);
                label.set_margin_end(12);
                label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                row.set_child(Some(&label));
            }
            ClipEntry::Image { path, mime } => {
                let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                hbox.add_css_class("psh-clip-entry-box");
                hbox.set_margin_top(6);
                hbox.set_margin_bottom(6);
                hbox.set_margin_start(12);
                hbox.set_margin_end(12);

                if path.exists() {
                    let image = gtk4::Image::from_file(path);
                    image.set_pixel_size(48);
                    image.add_css_class("psh-clip-image-thumb");
                    hbox.append(&image);
                }

                let label = gtk4::Label::new(Some(&format!(
                    "[{}] {}",
                    mime,
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )));
                label.set_halign(gtk4::Align::Start);
                label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                hbox.append(&label);

                row.set_child(Some(&hbox));
            }
        }
        list_box.append(&row);
    }

    // Auto-select first row
    if let Some(first) = list_box.row_at_index(0) {
        list_box.select_row(Some(&first));
    }
}
