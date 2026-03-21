#![allow(dead_code, unused_imports)]

mod modules;

use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;
use psh_core::config::{self, BarPosition};

fn main() {
    psh_core::logging::init("psh_bar");
    tracing::info!("starting psh-bar");

    let cfg = config::load().expect("failed to load config");
    let bar_cfg = cfg.bar.clone();

    let app = gtk4::Application::builder()
        .application_id("com.psh.bar")
        .build();

    app.connect_activate(move |app| {
        psh_core::theme::apply_theme(&cfg.theme.name);

        // Start IPC hub on background thread
        std::thread::spawn(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(e) = run_ipc_hub().await {
                    tracing::error!("IPC hub error: {e}");
                }
            });
        });

        let window = gtk4::ApplicationWindow::builder()
            .application(app)
            .build();

        window.init_layer_shell();
        window.set_layer(gtk4_layer_shell::Layer::Top);
        window.set_anchor(gtk4_layer_shell::Edge::Left, true);
        window.set_anchor(gtk4_layer_shell::Edge::Right, true);

        match bar_cfg.position {
            BarPosition::Top => window.set_anchor(gtk4_layer_shell::Edge::Top, true),
            BarPosition::Bottom => window.set_anchor(gtk4_layer_shell::Edge::Bottom, true),
        }

        let height = bar_cfg.height.unwrap_or(32);
        window.set_default_height(height as i32);
        window.set_namespace(Some("psh-bar"));

        let bar = gtk4::CenterBox::new();
        bar.add_css_class("psh-bar");

        // Left modules
        let left = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        left.set_margin_start(8);
        left.add_css_class("psh-bar-left");
        left.append(&modules::workspaces::widget());
        bar.set_start_widget(Some(&left));

        // Center modules
        let center = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        center.add_css_class("psh-bar-center");
        center.append(&modules::clock::widget());
        bar.set_center_widget(Some(&center));

        // Right modules
        let right = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        right.set_margin_end(8);
        right.add_css_class("psh-bar-right");
        right.append(&modules::battery::widget());
        bar.set_end_widget(Some(&right));

        window.set_child(Some(&bar));
        window.present();
    });

    app.run_with_args::<String>(&[]);
}

async fn run_ipc_hub() -> psh_core::Result<()> {
    use std::sync::Arc;
    use tokio::net::unix::OwnedWriteHalf;
    use tokio::sync::Mutex;

    let listener = psh_core::ipc::bind().await?;
    tracing::info!("IPC hub listening");

    let clients: Arc<Mutex<Vec<OwnedWriteHalf>>> = Arc::new(Mutex::new(Vec::new()));

    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|e| psh_core::PshError::Ipc(e.to_string()))?;

        let (mut read_half, write_half) = stream.into_split();

        {
            let mut cl = clients.lock().await;
            cl.push(write_half);
            tracing::debug!("client connected, total: {}", cl.len());
        }

        let clients_clone = clients.clone();
        tokio::spawn(async move {
            while let Ok(msg) = psh_core::ipc::recv_from(&mut read_half).await {
                tracing::debug!("hub received: {msg:?}");
                match &msg {
                    psh_core::ipc::Message::Ping => {
                        // Pong is a direct reply, not broadcast. We need the
                        // write half for this client, but it's in the shared vec.
                        // For now, broadcast Pong — the client will ignore extra messages.
                        broadcast(&clients_clone, &psh_core::ipc::Message::Pong).await;
                    }
                    psh_core::ipc::Message::NotificationCount { .. }
                    | psh_core::ipc::Message::ToggleLauncher
                    | psh_core::ipc::Message::ShowClipboardHistory
                    | psh_core::ipc::Message::ConfigReloaded
                    | psh_core::ipc::Message::SetWallpaper { .. }
                    | psh_core::ipc::Message::LockScreen => {
                        broadcast(&clients_clone, &msg).await;
                    }
                    _ => {}
                }
            }
            tracing::debug!("client disconnected");
        });
    }
}

/// Send a message to all connected clients, removing any that have disconnected.
async fn broadcast(
    clients: &tokio::sync::Mutex<Vec<tokio::net::unix::OwnedWriteHalf>>,
    msg: &psh_core::ipc::Message,
) {
    let mut cl = clients.lock().await;
    let mut i = 0;
    while i < cl.len() {
        if psh_core::ipc::send_to(&mut cl[i], msg).await.is_err() {
            tracing::debug!("removing disconnected client");
            cl.swap_remove(i);
        } else {
            i += 1;
        }
    }
}
