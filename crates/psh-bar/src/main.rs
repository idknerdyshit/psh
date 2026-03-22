//! psh-bar — System status bar and IPC hub for the psh desktop environment.
//!
//! Renders a GTK4 layer-shell panel with configurable modules (workspaces, clock,
//! battery, volume, network, tray, etc.) and acts as the central IPC message hub
//! that all other psh components connect to.

mod modules;
mod niri;

use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;
use psh_core::config::{self, BarConfig, BarPosition};
use psh_core::ipc::Message;

fn main() {
    psh_core::logging::init("psh_bar");
    tracing::info!("starting psh-bar");

    let cfg = config::load().expect("failed to load config");
    let bar_cfg = cfg.bar.clone();

    // Create shared tokio runtime for all async backends (IPC hub, modules)
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    let rt_handle = rt.handle().clone();

    // Start config file watcher
    let config_path = config::config_path();
    let _config_watcher = rt_handle.block_on(async {
        match config::watch(config_path) {
            Ok((tx, watcher)) => {
                // Broadcast ConfigReloaded to IPC clients on config change
                let mut rx = tx.subscribe();
                tokio::spawn(async move {
                    while rx.recv().await.is_ok() {
                        tracing::info!(
                            "config changed, broadcasting ConfigReloaded to IPC clients"
                        );
                        if let Ok(mut stream) = psh_core::ipc::connect().await {
                            let _ =
                                psh_core::ipc::send(&mut stream, &Message::ConfigReloaded).await;
                        }
                    }
                });
                Some(watcher)
            }
            Err(e) => {
                tracing::warn!("failed to watch config file: {e}");
                None
            }
        }
    });

    // Keep the runtime alive on a background thread
    std::thread::spawn(move || {
        rt.block_on(std::future::pending::<()>());
    });

    let app = gtk4::Application::builder()
        .application_id("com.psh.bar")
        .build();

    app.connect_activate(move |app| {
        psh_core::theme::apply_theme(&cfg.theme.name);

        // Watch theme CSS for live reload
        let theme_name = cfg.theme.name.clone();
        if let Some((tx, watcher)) = psh_core::theme::watch(&theme_name) {
            let mut rx = tx.subscribe();
            gtk4::glib::spawn_future_local(async move {
                while rx.recv().await.is_ok() {
                    psh_core::theme::apply_theme(&theme_name);
                }
            });
            // Keep the watcher alive for the process lifetime.
            std::mem::forget(watcher);
        }

        // Channel: modules -> IPC hub (outbound to clients)
        let (outbound_tx, outbound_rx) = async_channel::bounded::<Message>(64);

        // Per-module inbound channels: IPC hub -> modules
        let mut inbound_senders: Vec<async_channel::Sender<Message>> = Vec::new();

        // Build module lists from config or defaults
        let left_names = module_names(&bar_cfg.modules_left, modules::DEFAULT_LEFT);
        let center_names = module_names(&bar_cfg.modules_center, modules::DEFAULT_CENTER);
        let right_names = module_names(&bar_cfg.modules_right, modules::DEFAULT_RIGHT);

        // Build sections
        let left = build_section(
            &left_names,
            &bar_cfg,
            &outbound_tx,
            &mut inbound_senders,
            &rt_handle,
        );
        left.set_margin_start(8);
        left.add_css_class("psh-bar-left");

        let center = build_section(
            &center_names,
            &bar_cfg,
            &outbound_tx,
            &mut inbound_senders,
            &rt_handle,
        );
        center.add_css_class("psh-bar-center");

        let right = build_section(
            &right_names,
            &bar_cfg,
            &outbound_tx,
            &mut inbound_senders,
            &rt_handle,
        );
        right.set_margin_end(8);
        right.add_css_class("psh-bar-right");

        // Spawn IPC hub on the shared runtime
        rt_handle.spawn(async move {
            if let Err(e) = run_ipc_hub(outbound_rx, inbound_senders).await {
                tracing::error!("IPC hub error: {e}");
            }
        });

        let window = gtk4::ApplicationWindow::builder().application(app).build();

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
        bar.set_start_widget(Some(&left));
        bar.set_center_widget(Some(&center));
        bar.set_end_widget(Some(&right));

        window.set_child(Some(&bar));
        window.present();
    });

    app.connect_shutdown(|_| {
        tracing::info!("psh-bar shutting down");
        if let Some(path) = psh_core::ipc::socket_path().ok().filter(|p| p.exists()) {
            let _ = std::fs::remove_file(&path);
        }
    });

    app.run_with_args::<String>(&[]);
}

/// Resolve module names from config, falling back to defaults if config is empty.
fn module_names<'a>(configured: &'a [String], defaults: &'a [&'a str]) -> Vec<&'a str> {
    if configured.is_empty() {
        defaults.to_vec()
    } else {
        configured.iter().map(|s| s.as_str()).collect()
    }
}

/// Build a horizontal box of modules for one bar section.
fn build_section(
    names: &[&str],
    config: &BarConfig,
    outbound_tx: &async_channel::Sender<Message>,
    inbound_senders: &mut Vec<async_channel::Sender<Message>>,
    rt: &tokio::runtime::Handle,
) -> gtk4::Box {
    let section = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);

    for name in names {
        if let Some(module) = modules::create_module(name) {
            let (tx, rx) = async_channel::bounded::<Message>(16);
            inbound_senders.push(tx);

            let ctx = modules::ModuleContext {
                ipc_tx: outbound_tx.clone(),
                ipc_rx: rx,
                config: config.clone(),
                rt: rt.clone(),
            };

            tracing::debug!("loaded module: {}", module.name());
            section.append(&module.widget(&ctx));
        }
    }

    section
}

/// Run the IPC hub on the tokio runtime.
///
/// The hub:
/// - Binds the Unix socket and accepts client connections
/// - Forwards messages from IPC clients to all module inbound channels
/// - Forwards messages from the outbound channel to all IPC clients
/// - Proactively removes disconnected clients via a removal channel
async fn run_ipc_hub(
    outbound_rx: async_channel::Receiver<Message>,
    inbound_senders: Vec<async_channel::Sender<Message>>,
) -> psh_core::Result<()> {
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::net::unix::OwnedWriteHalf;
    use tokio::sync::Mutex;

    let listener = psh_core::ipc::bind().await?;
    tracing::info!("IPC hub listening");

    let clients: Arc<Mutex<HashMap<u64, OwnedWriteHalf>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut next_id: u64 = 0;

    // Channel for read tasks to signal client disconnection
    let (remove_tx, mut remove_rx) = tokio::sync::mpsc::unbounded_channel::<u64>();

    // Task: process removal signals from read tasks
    let clients_for_removal = clients.clone();
    tokio::spawn(async move {
        while let Some(id) = remove_rx.recv().await {
            let mut cl = clients_for_removal.lock().await;
            if cl.remove(&id).is_some() {
                tracing::debug!("removed disconnected client {id}, remaining: {}", cl.len());
            }
        }
    });

    // Task: forward module outbound messages to all IPC clients
    let clients_for_outbound = clients.clone();
    tokio::spawn(async move {
        while let Ok(msg) = outbound_rx.recv().await {
            tracing::debug!("hub outbound: {msg:?}");
            broadcast(&clients_for_outbound, &msg).await;
        }
    });

    // Accept loop: handle incoming client connections
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|e| psh_core::PshError::Ipc(e.to_string()))?;

        let (mut read_half, write_half) = stream.into_split();
        let client_id = next_id;
        next_id += 1;

        {
            let mut cl = clients.lock().await;
            cl.insert(client_id, write_half);
            tracing::debug!("client {client_id} connected, total: {}", cl.len());
        }

        let clients_clone = clients.clone();
        let inbound_senders = inbound_senders.clone();
        let remove_tx = remove_tx.clone();
        tokio::spawn(async move {
            while let Ok(msg) = psh_core::ipc::recv_from(&mut read_half).await {
                tracing::debug!("hub received from client {client_id}: {msg:?}");

                // Fan out to module inbound channels
                for tx in &inbound_senders {
                    if tx.send(msg.clone()).await.is_err() {
                        tracing::debug!("module inbound channel closed");
                    }
                }

                match &msg {
                    Message::Ping => {
                        send_to_client(&clients_clone, client_id, &Message::Pong).await;
                    }
                    Message::NotificationCount { .. }
                    | Message::ToggleLauncher
                    | Message::ShowClipboardHistory
                    | Message::ConfigReloaded
                    | Message::SetWallpaper { .. }
                    | Message::LockScreen => {
                        broadcast(&clients_clone, &msg).await;
                    }
                    _ => {}
                }
            }
            tracing::debug!("client {client_id} disconnected");
            let _ = remove_tx.send(client_id);
        });
    }
}

/// Send a message to a single client by ID. Removes the client on write failure.
async fn send_to_client(
    clients: &tokio::sync::Mutex<std::collections::HashMap<u64, tokio::net::unix::OwnedWriteHalf>>,
    client_id: u64,
    msg: &Message,
) {
    let mut cl = clients.lock().await;
    if let Some(writer) = cl.get_mut(&client_id)
        && psh_core::ipc::send_to(writer, msg).await.is_err()
    {
        tracing::debug!("removing client {client_id} after write failure");
        cl.remove(&client_id);
    }
}

/// Send a message to all connected clients, removing any that fail to write.
async fn broadcast(
    clients: &tokio::sync::Mutex<std::collections::HashMap<u64, tokio::net::unix::OwnedWriteHalf>>,
    msg: &Message,
) {
    let mut cl = clients.lock().await;
    let mut failed: Vec<u64> = Vec::new();
    for (id, writer) in cl.iter_mut() {
        if psh_core::ipc::send_to(writer, msg).await.is_err() {
            failed.push(*id);
        }
    }
    for id in failed {
        tracing::debug!("removing client {id} after write failure");
        cl.remove(&id);
    }
}
