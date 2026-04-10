//! psh-bar — System status bar and IPC hub for the psh desktop environment.
//!
//! Renders a GTK4 layer-shell panel with configurable modules (workspaces, clock,
//! battery, volume, network, tray, etc.) and acts as the central IPC message hub
//! that all other psh components connect to.

mod modules;
mod niri;
mod wayland;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex as StdMutex};

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
        // Guard against re-activation creating duplicate bars and IPC hubs
        if !app.windows().is_empty() {
            tracing::debug!("already running, ignoring re-activation");
            return;
        }

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

        // Per-module inbound channels shared with the IPC hub and hotplug handler
        let inbound_senders: Arc<StdMutex<Vec<async_channel::Sender<Message>>>> =
            Arc::new(StdMutex::new(Vec::new()));

        // Enumerate monitors and create one bar window per output.
        let display = gtk4::gdk::Display::default().expect("no GDK display");
        let monitors = display.monitors();
        let n_monitors = monitors.n_items();

        // Track bar windows by monitor connector for hotplug management
        let windows: Rc<RefCell<HashMap<String, gtk4::ApplicationWindow>>> =
            Rc::new(RefCell::new(HashMap::new()));

        if n_monitors > 0 {
            for i in 0..n_monitors {
                if let Some(monitor) = monitors
                    .item(i)
                    .and_then(|obj| obj.downcast::<gtk4::gdk::Monitor>().ok())
                {
                    let key = monitor_connector(&monitor, i);
                    let window = create_bar_window(
                        app,
                        Some(&monitor),
                        &bar_cfg,
                        &outbound_tx,
                        &inbound_senders,
                        &rt_handle,
                    );
                    windows.borrow_mut().insert(key, window);
                }
            }
        } else {
            // No monitors detected (unlikely) — create one unassigned bar
            create_bar_window(
                app,
                None,
                &bar_cfg,
                &outbound_tx,
                &inbound_senders,
                &rt_handle,
            );
        }

        // Handle monitor hotplug: add/remove bar windows as monitors appear/disappear
        {
            let windows = windows.clone();
            let app = app.downgrade();
            let bar_cfg = bar_cfg.clone();
            let outbound_tx = outbound_tx.clone();
            let inbound_senders = inbound_senders.clone();
            let rt_handle = rt_handle.clone();

            monitors.connect_items_changed(move |model, _pos, _removed, _added| {
                let Some(app) = app.upgrade() else {
                    return;
                };
                let mut wins = windows.borrow_mut();

                // Build current monitor set
                let mut current: HashMap<String, gtk4::gdk::Monitor> = HashMap::new();
                for i in 0..model.n_items() {
                    if let Some(mon) = model
                        .item(i)
                        .and_then(|o| o.downcast::<gtk4::gdk::Monitor>().ok())
                    {
                        current.insert(monitor_connector(&mon, i), mon);
                    }
                }

                // Close bars for disconnected monitors
                wins.retain(|conn, window| {
                    if current.contains_key(conn) {
                        true
                    } else {
                        tracing::info!("monitor removed: {conn}, closing bar");
                        window.close();
                        false
                    }
                });

                // Create bars for newly connected monitors
                for (conn, mon) in &current {
                    if !wins.contains_key(conn) {
                        tracing::info!("monitor added: {conn}, creating bar");
                        let window = create_bar_window(
                            &app,
                            Some(mon),
                            &bar_cfg,
                            &outbound_tx,
                            &inbound_senders,
                            &rt_handle,
                        );
                        wins.insert(conn.clone(), window);
                    }
                }
            });
        }

        // Spawn IPC hub on the shared runtime
        rt_handle.spawn(async move {
            if let Err(e) = run_ipc_hub(outbound_rx, inbound_senders).await {
                tracing::error!("IPC hub error: {e}");
            }
        });
    });

    app.connect_shutdown(|_| {
        tracing::info!("psh-bar shutting down");
        if let Some(path) = psh_core::ipc::socket_path().ok().filter(|p| p.exists()) {
            let _ = std::fs::remove_file(&path);
        }
    });

    app.run_with_args::<String>(&[]);
}

/// Get a stable identifier for a GDK monitor, preferring its connector name.
fn monitor_connector(monitor: &gtk4::gdk::Monitor, index: u32) -> String {
    monitor
        .connector()
        .map(|c| c.to_string())
        .unwrap_or_else(|| format!("monitor-{index}"))
}

/// Resolve module names from config, falling back to defaults if config is empty.
fn module_names<'a>(configured: &'a [String], defaults: &'a [&'a str]) -> Vec<&'a str> {
    if configured.is_empty() {
        defaults.to_vec()
    } else {
        configured.iter().map(|s| s.as_str()).collect()
    }
}

/// Create a bar window with all modules, optionally assigned to a specific monitor.
fn create_bar_window(
    app: &gtk4::Application,
    monitor: Option<&gtk4::gdk::Monitor>,
    bar_cfg: &BarConfig,
    outbound_tx: &async_channel::Sender<Message>,
    inbound_senders: &Arc<StdMutex<Vec<async_channel::Sender<Message>>>>,
    rt_handle: &tokio::runtime::Handle,
) -> gtk4::ApplicationWindow {
    let left_names = module_names(&bar_cfg.modules_left, modules::DEFAULT_LEFT);
    let center_names = module_names(&bar_cfg.modules_center, modules::DEFAULT_CENTER);
    let right_names = module_names(&bar_cfg.modules_right, modules::DEFAULT_RIGHT);
    let height = bar_cfg.height.unwrap_or(32) as i32;

    let mut local_senders = Vec::new();

    let left = build_section(
        &left_names,
        bar_cfg,
        outbound_tx,
        &mut local_senders,
        rt_handle,
    );
    left.set_margin_start(8);
    left.add_css_class("psh-bar-left");

    let center = build_section(
        &center_names,
        bar_cfg,
        outbound_tx,
        &mut local_senders,
        rt_handle,
    );
    center.add_css_class("psh-bar-center");

    let right = build_section(
        &right_names,
        bar_cfg,
        outbound_tx,
        &mut local_senders,
        rt_handle,
    );
    right.set_margin_end(8);
    right.add_css_class("psh-bar-right");

    // Register new module channels with the IPC hub
    inbound_senders.lock().unwrap().extend(local_senders);

    let bar = gtk4::CenterBox::new();
    bar.add_css_class("psh-bar");
    bar.set_start_widget(Some(&left));
    bar.set_center_widget(Some(&center));
    bar.set_end_widget(Some(&right));

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

    window.set_default_height(height);
    window.auto_exclusive_zone_enable();
    window.set_namespace(Some("psh-bar"));

    if let Some(mon) = monitor {
        window.set_monitor(Some(mon));
    }

    window.set_child(Some(&bar));
    window.present();

    window
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
    inbound_senders: Arc<StdMutex<Vec<async_channel::Sender<Message>>>>,
) -> psh_core::Result<()> {
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

                // Fan out to module inbound channels (snapshot under lock, then send)
                let senders = inbound_senders.lock().unwrap().clone();
                for tx in &senders {
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
    clients: &tokio::sync::Mutex<HashMap<u64, tokio::net::unix::OwnedWriteHalf>>,
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
    clients: &tokio::sync::Mutex<HashMap<u64, tokio::net::unix::OwnedWriteHalf>>,
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
