//! psh-launch — application launcher daemon for the psh desktop environment.
//!
//! Runs as a long-lived process with a hidden GTK4 layer-shell overlay window.
//! Listens for `ToggleLauncher` IPC messages to show/hide the launcher. Provides
//! fuzzy search over `.desktop` files with frecency-based result ordering and
//! supports launching both graphical and terminal applications.

mod desktop;
mod frecency;

use std::cell::RefCell;
use std::rc::Rc;

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
    let terminal_cmd = cfg.launch.terminal.clone();
    let theme_name = cfg.theme.name.clone();

    let app = gtk4::Application::builder()
        .application_id("com.psh.launch")
        .build();

    let window_ref: Rc<RefCell<Option<gtk4::ApplicationWindow>>> = Rc::new(RefCell::new(None));

    let window_ref_activate = window_ref.clone();
    app.connect_activate(move |app| {
        // If window already exists, toggle it and return (single-instance).
        if let Some(ref win) = *window_ref_activate.borrow() {
            toggle_window(win);
            return;
        }

        psh_core::theme::apply_theme(&theme_name);

        let tracker = Rc::new(RefCell::new(frecency::FrecencyTracker::load()));
        let entries = Rc::new(RefCell::new(desktop::load_desktop_entries()));
        let matcher = Rc::new(RefCell::new(Matcher::new(Config::DEFAULT)));
        tracing::info!("loaded {} desktop entries", entries.borrow().len());

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

        populate_list(&list_box, &entries.borrow(), &tracker.borrow(), &mut matcher.borrow_mut(), "", max_results);

        let entries_search = entries.clone();
        let tracker_search = tracker.clone();
        let matcher_search = matcher.clone();
        let list_box_search = list_box.clone();
        search_entry.connect_search_changed(move |entry| {
            let query = entry.text().to_string();
            populate_list(
                &list_box_search,
                &entries_search.borrow(),
                &tracker_search.borrow(),
                &mut matcher_search.borrow_mut(),
                &query,
                max_results,
            );
        });

        let window_launch = window.clone();
        let tracker_launch = tracker.clone();
        let entries_launch = entries.clone();
        let terminal_cmd_clone = terminal_cmd.clone();
        list_box.connect_row_activated(move |_, row| {
            let Some(idx) = row
                .widget_name()
                .strip_prefix("idx:")
                .and_then(|s| s.parse::<usize>().ok())
            else {
                return;
            };

            let borrowed = entries_launch.borrow();
            let Some(entry) = borrowed.get(idx) else {
                return;
            };

            launch_app(entry, terminal_cmd_clone.as_deref());
            tracker_launch.borrow_mut().record(&entry.exec);
            hide_window(&window_launch);
        });

        let key_controller = gtk4::EventControllerKey::new();
        let window_key = window.clone();
        let list_box_key = list_box.clone();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            match key {
                gtk4::gdk::Key::Escape => {
                    hide_window(&window_key);
                    glib::Propagation::Stop
                }
                gtk4::gdk::Key::Return | gtk4::gdk::Key::KP_Enter => {
                    if let Some(row) = list_box_key.selected_row() {
                        row.activate();
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        window.add_controller(key_controller);

        window.set_child(Some(&container));

        *window_ref_activate.borrow_mut() = Some(window.clone());

        let search_entry_ref = search_entry.clone();
        let list_box_ref = list_box.clone();
        let entries_ref = entries.clone();
        let tracker_ref = tracker.clone();
        let matcher_ref = matcher.clone();

        // IPC client: listen for ToggleLauncher on a background thread with auto-reconnect.
        let (tx, rx) = async_channel::bounded::<()>(4);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                loop {
                    match psh_core::ipc::connect().await {
                        Ok(mut stream) => {
                            tracing::info!("connected to IPC hub");
                            loop {
                                match psh_core::ipc::recv(&mut stream).await {
                                    Ok(psh_core::ipc::Message::ToggleLauncher) => {
                                        let _ = tx.send(()).await;
                                    }
                                    Ok(_) => {}
                                    Err(e) => {
                                        tracing::warn!("IPC recv failed: {e}, reconnecting");
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::debug!("IPC connect failed: {e}, retrying in 5s");
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            });
        });

        // Receive toggle signals on the GTK thread.
        let window_ipc = window.clone();
        glib::spawn_future_local(async move {
            while let Ok(()) = rx.recv().await {
                if !window_ipc.is_visible() {
                    *entries_ref.borrow_mut() = desktop::load_desktop_entries();
                    populate_list(
                        &list_box_ref,
                        &entries_ref.borrow(),
                        &tracker_ref.borrow(),
                        &mut matcher_ref.borrow_mut(),
                        "",
                        max_results,
                    );
                    search_entry_ref.set_text("");
                    search_entry_ref.grab_focus();
                }
                toggle_window(&window_ipc);
            }
        });

        window.present();
        search_entry.grab_focus();
    });

    app.run_with_args::<String>(&[]);
}

/// Toggle the overlay window between visible and hidden states.
fn toggle_window(window: &gtk4::ApplicationWindow) {
    if window.is_visible() {
        hide_window(window);
    } else {
        window.present();
    }
}

/// Hide the window without destroying it.
fn hide_window(window: &gtk4::ApplicationWindow) {
    window.set_visible(false);
}

/// Populate the list box with entries, optionally filtered by a fuzzy query.
/// When the query is empty, entries are sorted by frecency score then alphabetically.
/// When a query is present, entries are sorted by combined fuzzy + frecency score.
fn populate_list(
    list_box: &gtk4::ListBox,
    entries: &[desktop::DesktopEntry],
    tracker: &frecency::FrecencyTracker,
    matcher: &mut Matcher,
    query: &str,
    max_results: usize,
) {
    while let Some(row) = list_box.row_at_index(0) {
        list_box.remove(&row);
    }

    let now = frecency::now_secs();

    if query.is_empty() {
        let mut sorted: Vec<_> = entries.iter().enumerate().collect();
        sorted.sort_by(|(_, a), (_, b)| {
            let sa = tracker.score_at(&a.exec, now);
            let sb = tracker.score_at(&b.exec, now);
            sb.partial_cmp(&sa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        for (idx, entry) in sorted.into_iter().take(max_results) {
            list_box.append(&create_entry_row(idx, entry));
        }
    } else {
        let pattern =
            Pattern::new(query, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy);

        let mut scored: Vec<_> = entries
            .iter()
            .enumerate()
            .filter_map(|(idx, e)| {
                let mut buf = Vec::new();
                let haystack = nucleo_matcher::Utf32Str::new(&e.name, &mut buf);
                pattern.score(haystack, matcher).map(|s| {
                    let frecency = tracker.score_at(&e.exec, now);
                    let combined = f64::from(s) + frecency;
                    (idx, e, combined)
                })
            })
            .collect();
        scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        for (idx, e, _) in scored.into_iter().take(max_results) {
            list_box.append(&create_entry_row(idx, e));
        }
    }

    if let Some(first) = list_box.row_at_index(0) {
        list_box.select_row(Some(&first));
    }
}

/// Create a list box row for a desktop entry, with optional icon.
/// The index refers to the entry's position in the entries vec for lookup on activation.
fn create_entry_row(idx: usize, entry: &desktop::DesktopEntry) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.set_widget_name(&format!("idx:{idx}"));

    let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    hbox.set_margin_top(8);
    hbox.set_margin_bottom(8);
    hbox.set_margin_start(12);
    hbox.set_margin_end(12);

    if let Some(ref icon_name) = entry.icon {
        let icon = gtk4::Image::from_icon_name(icon_name);
        icon.set_pixel_size(24);
        icon.add_css_class("psh-launch-icon");
        hbox.append(&icon);
    }

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

/// Launch an application, respecting the Terminal flag.
fn launch_app(entry: &desktop::DesktopEntry, terminal_cmd: Option<&str>) {
    let mut cmd = if entry.terminal {
        let term = terminal_cmd
            .map(|s| s.to_string())
            .or_else(detect_terminal);
        if let Some(t) = term {
            tracing::info!("launching terminal app: {t} -e sh -c {}", entry.exec);
            let mut c = std::process::Command::new(t);
            c.arg("-e").arg("sh").arg("-c").arg(&entry.exec);
            c
        } else {
            tracing::warn!("no terminal found, launching directly: {}", entry.exec);
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(&entry.exec);
            c
        }
    } else {
        tracing::info!("launching: {}", entry.exec);
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(&entry.exec);
        c
    };

    if let Err(e) = cmd.spawn() {
        tracing::error!("failed to launch {}: {e}", entry.exec);
    }
}

/// Try to find a terminal emulator on the system.
fn detect_terminal() -> Option<String> {
    for candidate in ["foot", "alacritty", "kitty", "wezterm", "xterm"] {
        if which(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Check if a command is available on PATH.
fn which(cmd: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|dir| std::path::Path::new(dir).join(cmd).is_file())
}
