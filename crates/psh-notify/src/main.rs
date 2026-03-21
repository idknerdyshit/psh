mod dbus_server;
mod manager;

use std::time::Duration;

use gtk4::glib;
use gtk4::prelude::*;
use psh_core::config;
use psh_core::ipc;

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

        // Forward channel: D-Bus thread → GTK thread
        let (dbus_tx, dbus_rx) = async_channel::bounded::<dbus_server::DbusToGtk>(32);
        // Reverse channel: GTK thread → D-Bus thread (for signal emission)
        let (signal_tx, signal_rx) = async_channel::bounded::<dbus_server::GtkToDbus>(32);
        // IPC count channel: GTK thread → tokio thread
        let (ipc_count_tx, ipc_count_rx) = async_channel::bounded::<u32>(4);

        // Background thread: D-Bus server + IPC client
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                tokio::spawn(run_ipc_client(ipc_count_rx));

                if let Err(e) = dbus_server::run(dbus_tx, signal_rx).await {
                    tracing::error!("dbus server error: {e}");
                }
            });
        });

        // Create notification manager
        let manager = manager::NotificationManager::new(
            app.clone(),
            notify_cfg.clone(),
            signal_tx,
            ipc_count_tx,
        );

        // Dispatch D-Bus messages to the manager on the GTK thread
        glib::spawn_future_local(async move {
            while let Ok(msg) = dbus_rx.recv().await {
                manager::NotificationManager::handle(&manager, msg);
            }
        });
    });

    app.run_with_args::<String>(&[]);
}

/// IPC client that sends NotificationCount updates to the psh-bar hub.
/// Runs on the tokio background thread with automatic reconnection.
async fn run_ipc_client(count_rx: async_channel::Receiver<u32>) {
    loop {
        match ipc::connect().await {
            Ok(mut stream) => {
                tracing::info!("connected to IPC hub");
                while let Ok(count) = count_rx.recv().await {
                    let msg = ipc::Message::NotificationCount { count };
                    if let Err(e) = ipc::send(&mut stream, &msg).await {
                        tracing::warn!("IPC send failed: {e}, reconnecting");
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::debug!("IPC connect failed: {e}, retrying in 5s");
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
