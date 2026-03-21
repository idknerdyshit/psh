#![allow(dead_code, unused_imports)]

mod agent;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;

fn main() {
    psh_core::logging::init("psh_polkit");
    tracing::info!("starting psh-polkit");

    let cfg = psh_core::config::load().expect("failed to load config");

    let app = gtk4::Application::builder()
        .application_id("com.psh.polkit")
        .build();

    app.connect_activate(move |app| {
        psh_core::theme::apply_theme(&cfg.theme.name);

        let (tx, rx) = async_channel::bounded::<agent::AuthRequest>(4);
        let (resp_tx, resp_rx) = async_channel::bounded::<agent::AuthResponse>(4);

        // Start polkit agent on background thread
        let tx_clone = tx.clone();
        let resp_rx_clone = resp_rx.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(e) = agent::run(tx_clone, resp_rx_clone).await {
                    tracing::error!("polkit agent error: {e}");
                }
            });
        });

        let app_clone = app.clone();
        let resp_tx_clone = resp_tx.clone();
        glib::spawn_future_local(async move {
            while let Ok(req) = rx.recv().await {
                show_auth_dialog(&app_clone, &req, &resp_tx_clone);
            }
        });
    });

    app.run_with_args::<String>(&[]);
}

fn show_auth_dialog(
    app: &gtk4::Application,
    req: &agent::AuthRequest,
    resp_tx: &async_channel::Sender<agent::AuthResponse>,
) {
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .default_width(400)
        .default_height(200)
        .decorated(false)
        .build();

    window.init_layer_shell();
    window.set_layer(gtk4_layer_shell::Layer::Overlay);
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::Exclusive);

    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    container.add_css_class("psh-polkit-dialog");
    container.set_margin_top(24);
    container.set_margin_bottom(24);
    container.set_margin_start(24);
    container.set_margin_end(24);

    let title = gtk4::Label::new(Some("Authentication Required"));
    title.add_css_class("psh-polkit-title");
    container.append(&title);

    let message = gtk4::Label::new(Some(&req.message));
    message.set_wrap(true);
    container.append(&message);

    let entry = gtk4::PasswordEntry::new();
    entry.set_show_peek_icon(true);
    container.append(&entry);

    let button_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    button_box.set_halign(gtk4::Align::End);

    let cancel_btn = gtk4::Button::with_label("Cancel");
    let auth_btn = gtk4::Button::with_label("Authenticate");
    auth_btn.add_css_class("suggested-action");

    button_box.append(&cancel_btn);
    button_box.append(&auth_btn);
    container.append(&button_box);

    window.set_child(Some(&container));

    let window_cancel = window.clone();
    let resp_tx_cancel = resp_tx.clone();
    let cookie_cancel = req.cookie.clone();
    cancel_btn.connect_clicked(move |_| {
        let _ = resp_tx_cancel.send_blocking(agent::AuthResponse {
            cookie: cookie_cancel.clone(),
            password: None,
        });
        window_cancel.close();
    });

    let window_auth = window.clone();
    let resp_tx_auth = resp_tx.clone();
    let cookie_auth = req.cookie.clone();
    auth_btn.connect_clicked(move |_| {
        let password = entry.text().to_string();
        let _ = resp_tx_auth.send_blocking(agent::AuthResponse {
            cookie: cookie_auth.clone(),
            password: Some(password),
        });
        window_auth.close();
    });

    window.present();
}
