//! Tray module — displays system tray items via the StatusNotifierItem protocol.
//!
//! Uses the `system-tray` crate to implement the StatusNotifierWatcher D-Bus
//! service and collect tray items from running applications.

use gtk4::glib;
use gtk4::prelude::*;

use super::{BarModule, ModuleContext};

/// A snapshot of a tray item for rendering.
#[derive(Debug, Clone)]
pub(crate) struct TrayItemInfo {
    /// The SNI item's bus address (used as key).
    pub address: String,
    /// Display title from the item.
    pub title: Option<String>,
    /// Freedesktop icon name, if any.
    pub icon_name: Option<String>,
    /// ARGB32 icon pixmap (width, height, pixels) — largest available.
    pub icon_pixmap: Option<(i32, i32, Vec<u8>)>,
}

/// Events sent from the tray backend to the GTK thread.
#[derive(Debug, Clone)]
enum TrayUpdate {
    /// An item was added or updated.
    AddOrUpdate(TrayItemInfo),
    /// An item was removed.
    Remove(String),
}

/// Commands sent from the GTK thread to the tray backend.
#[derive(Debug)]
enum TrayCommand {
    /// Activate the tray item at the given address.
    Activate(String),
}

/// Displays system tray icons from applications using the SNI protocol.
///
/// Each tray item is rendered as a button with an icon. Click activates the item.
pub struct TrayModule;

impl BarModule for TrayModule {
    fn name(&self) -> &'static str {
        "tray"
    }

    fn widget(&self, ctx: &ModuleContext) -> gtk4::Widget {
        let container = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        container.add_css_class("psh-bar-tray");

        let (tx, rx) = async_channel::bounded::<TrayUpdate>(32);
        let (cmd_tx, cmd_rx) = async_channel::bounded::<TrayCommand>(8);

        // Background task: run the SNI watcher and relay events on the shared runtime
        ctx.rt.spawn(async move {
            if let Err(e) = run_tray_backend(tx, cmd_rx).await {
                tracing::error!("tray backend error: {e}");
            }
        });

        // GTK side: add/update/remove tray item buttons
        let container_clone = container.clone();
        glib::spawn_future_local(async move {
            while let Ok(update) = rx.recv().await {
                match update {
                    TrayUpdate::AddOrUpdate(info) => {
                        add_or_update_tray_item(&container_clone, &info, &cmd_tx);
                    }
                    TrayUpdate::Remove(address) => {
                        remove_tray_item(&container_clone, &address);
                    }
                }
            }
        });

        container.upcast()
    }
}

/// Run the system-tray backend: register the StatusNotifierWatcher and relay events.
///
/// Also listens for activation commands from the GTK thread.
async fn run_tray_backend(
    tx: async_channel::Sender<TrayUpdate>,
    cmd_rx: async_channel::Receiver<TrayCommand>,
) -> psh_core::Result<()> {
    let client = system_tray::client::Client::new()
        .await
        .map_err(|e| psh_core::PshError::Other(format!("tray client init failed: {e}")))?;

    let mut tray_rx = client.subscribe();

    // Send initial items — items() returns a map of (StatusNotifierItem, Option<TrayMenu>) tuples
    {
        let items = client.items();
        let items = items.lock().unwrap();
        for (address, (item, _menu)) in items.iter() {
            let info = tray_item_info(address, item);
            if tx.try_send(TrayUpdate::AddOrUpdate(info)).is_err() {
                tracing::debug!("tray update channel full during initial sync");
            }
        }
    }

    // Process tray events and activation commands
    loop {
        tokio::select! {
            event = tray_rx.recv() => {
                match event {
                    Ok(event) => handle_tray_event(&tx, &client, event).await,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!("tray event receiver lagged by {n}");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Ok(TrayCommand::Activate(address)) => {
                        let req = system_tray::client::ActivateRequest::Default {
                            address: address.clone(),
                            x: 0,
                            y: 0,
                        };
                        if let Err(e) = client.activate(req).await {
                            tracing::warn!("tray activate failed for {address}: {e}");
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    Ok(())
}

/// Handle a single tray event from the system-tray broadcast channel.
async fn handle_tray_event(
    tx: &async_channel::Sender<TrayUpdate>,
    client: &system_tray::client::Client,
    event: system_tray::client::Event,
) {
    match event {
        system_tray::client::Event::Add(address, item) => {
            let info = tray_item_info(&address, &item);
            let _ = tx.send(TrayUpdate::AddOrUpdate(info)).await;
        }
        system_tray::client::Event::Update(address, _update) => {
            let info = {
                let items = client.items();
                let items = items.lock().unwrap();
                items
                    .get(&address)
                    .map(|(item, _menu)| tray_item_info(&address, item))
            };
            if let Some(info) = info {
                let _ = tx.send(TrayUpdate::AddOrUpdate(info)).await;
            }
        }
        system_tray::client::Event::Remove(address) => {
            let _ = tx.send(TrayUpdate::Remove(address)).await;
        }
    }
}

/// Convert a StatusNotifierItem to our simplified TrayItemInfo.
fn tray_item_info(address: &str, item: &system_tray::item::StatusNotifierItem) -> TrayItemInfo {
    let icon_pixmap = item
        .icon_pixmap
        .as_ref()
        .and_then(|pixmaps| {
            // Pick the largest pixmap
            pixmaps
                .iter()
                .max_by_key(|p| (p.width as i64) * (p.height as i64))
        })
        .map(|p| (p.width, p.height, p.pixels.clone()));

    TrayItemInfo {
        address: address.to_string(),
        title: item.title.clone(),
        icon_name: item.icon_name.clone(),
        icon_pixmap,
    }
}

/// Add or update a tray item button in the container.
fn add_or_update_tray_item(
    container: &gtk4::Box,
    info: &TrayItemInfo,
    cmd_tx: &async_channel::Sender<TrayCommand>,
) {
    // Look for an existing button with this address
    let mut child = container.first_child();
    while let Some(widget) = child {
        if widget.widget_name() == info.address {
            // Update existing: remove and re-add
            container.remove(&widget);
            break;
        }
        child = widget.next_sibling();
    }

    let btn = gtk4::Button::new();
    btn.set_widget_name(&info.address);
    btn.add_css_class("psh-bar-tray-item");

    if let Some(ref tooltip) = info.title {
        btn.set_tooltip_text(Some(tooltip));
    }

    // Try icon name first (GTK theme lookup), fall back to pixmap
    if let Some(ref icon_name) = info.icon_name {
        if !icon_name.is_empty() {
            let image = gtk4::Image::from_icon_name(icon_name);
            image.set_pixel_size(20);
            btn.set_child(Some(&image));
        } else {
            set_pixmap_icon(&btn, info);
        }
    } else {
        set_pixmap_icon(&btn, info);
    }

    // Click to activate via the backend
    let cmd_tx = cmd_tx.clone();
    let address = info.address.clone();
    btn.connect_clicked(move |_| {
        if cmd_tx.try_send(TrayCommand::Activate(address.clone())).is_err() {
            tracing::debug!("tray command channel full (activate)");
        }
    });

    container.append(&btn);
}

/// Set a pixmap icon on a tray button from ARGB32 data.
fn set_pixmap_icon(btn: &gtk4::Button, info: &TrayItemInfo) {
    if let Some((width, height, pixels)) = info
        .icon_pixmap
        .as_ref()
        .filter(|(w, h, p)| *w > 0 && *h > 0 && p.len() as i32 >= w * h * 4)
    {
        // SNI pixmaps are ARGB32 in network byte order (big-endian).
        let bytes = glib::Bytes::from(pixels);
        let texture = gtk4::gdk::MemoryTexture::new(
            *width,
            *height,
            gtk4::gdk::MemoryFormat::A8r8g8b8,
            &bytes,
            (*width * 4) as usize,
        );
        let image = gtk4::Image::from_paintable(Some(&texture));
        image.set_pixel_size(20);
        btn.set_child(Some(&image));
    }

    // If no icon at all, show a placeholder
    if btn.child().is_none() {
        let label = gtk4::Label::new(Some("\u{25cf}")); // bullet
        btn.set_child(Some(&label));
    }
}

/// Remove a tray item button from the container by address.
fn remove_tray_item(container: &gtk4::Box, address: &str) {
    let mut child = container.first_child();
    while let Some(widget) = child {
        if widget.widget_name() == address {
            container.remove(&widget);
            return;
        }
        child = widget.next_sibling();
    }
}
