#![allow(dead_code, unused_imports)]

mod dbus_server;

use async_channel::Sender;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;
use psh_core::config;

fn main() {
    psh_core::logging::init("psh_notify");
    tracing::info!("starting psh-notify");

    let cfg = config::load().expect("failed to load config");
    let notify_cfg = cfg.notify.clone();

    let app = gtk4::Application::builder()
        .application_id("com.psh.notify")
        .build();

    app.connect_activate(move |app| {
        psh_core::theme::apply_theme(&cfg.theme.name);

        let (tx, rx) = async_channel::bounded::<dbus_server::Notification>(32);

        // Start D-Bus server on a background thread
        let tx_clone = tx.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(e) = dbus_server::run(tx_clone).await {
                    tracing::error!("dbus server error: {e}");
                }
            });
        });

        let app_clone = app.clone();
        let notify_cfg = notify_cfg.clone();
        glib::spawn_future_local(async move {
            while let Ok(notif) = rx.recv().await {
                show_notification(&app_clone, &notif, &notify_cfg);
            }
        });
    });

    app.run_with_args::<String>(&[]);
}

fn show_notification(
    app: &gtk4::Application,
    notif: &dbus_server::Notification,
    cfg: &config::NotifyConfig,
) {
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .default_width(380)
        .default_height(100)
        .decorated(false)
        .build();

    window.init_layer_shell();
    window.set_layer(gtk4_layer_shell::Layer::Overlay);
    window.set_anchor(gtk4_layer_shell::Edge::Top, true);
    window.set_anchor(gtk4_layer_shell::Edge::Right, true);
    window.set_margin(gtk4_layer_shell::Edge::Top, 10);
    window.set_margin(gtk4_layer_shell::Edge::Right, 10);

    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    container.add_css_class("psh-notify-popup");
    container.set_margin_top(12);
    container.set_margin_bottom(12);
    container.set_margin_start(16);
    container.set_margin_end(16);

    let summary = gtk4::Label::new(Some(&notif.summary));
    summary.add_css_class("psh-notify-summary");
    summary.set_halign(gtk4::Align::Start);
    summary.set_wrap(true);
    container.append(&summary);

    if !notif.body.is_empty() {
        let body = gtk4::Label::new(Some(&notif.body));
        body.add_css_class("psh-notify-body");
        body.set_halign(gtk4::Align::Start);
        body.set_wrap(true);
        container.append(&body);
    }

    window.set_child(Some(&container));
    window.present();

    let timeout = if notif.expire_timeout > 0 {
        notif.expire_timeout as u64
    } else {
        cfg.default_timeout_ms
    };

    let window_clone = window.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(timeout), move || {
        window_clone.close();
    });
}
