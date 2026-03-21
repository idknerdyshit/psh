#![allow(dead_code, unused_imports)]

mod history;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;
use psh_core::config;

fn main() {
    psh_core::logging::init("psh_clip");
    tracing::info!("starting psh-clip");

    let cfg = config::load().expect("failed to load config");
    let clip_cfg = cfg.clip.clone();

    let app = gtk4::Application::builder()
        .application_id("com.psh.clip")
        .build();

    app.connect_activate(move |app| {
        psh_core::theme::apply_theme(&cfg.theme.name);

        let history = history::ClipHistory::new(clip_cfg.max_history);

        // Listen for IPC ShowClipboardHistory commands on a background thread
        let history_clone = history.clone();
        let (tx, rx) = async_channel::bounded::<()>(4);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let Ok(mut stream) = psh_core::ipc::connect().await else {
                    tracing::warn!("could not connect to IPC hub, running standalone");
                    return;
                };
                loop {
                    match psh_core::ipc::recv(&mut stream).await {
                        Ok(psh_core::ipc::Message::ShowClipboardHistory) => {
                            let _ = tx.send(()).await;
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

        let app_clone = app.clone();
        glib::spawn_future_local(async move {
            while let Ok(()) = rx.recv().await {
                show_picker(&app_clone, &history_clone);
            }
        });
    });

    app.run_with_args::<String>(&[]);
}

fn show_picker(app: &gtk4::Application, history: &history::ClipHistory) {
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

    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_vexpand(true);

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("psh-clip-list");

    let items = history.items();
    for item in &items {
        let row = gtk4::ListBoxRow::new();
        let label = gtk4::Label::new(Some(&truncate(item, 80)));
        label.set_halign(gtk4::Align::Start);
        label.set_margin_top(6);
        label.set_margin_bottom(6);
        label.set_margin_start(12);
        label.set_margin_end(12);
        row.set_child(Some(&label));
        list_box.append(&row);
    }

    scroll.set_child(Some(&list_box));
    container.append(&scroll);

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
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
