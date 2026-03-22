//! Tray module — displays system tray items via the StatusNotifierItem protocol.
//!
//! Uses the `system-tray` crate to implement the StatusNotifierWatcher D-Bus
//! service and collect tray items from running applications. Click on a tray
//! icon shows its D-Bus menu (if available) as a GTK4 popover.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;

use system_tray::menu::{MenuItem, MenuType, ToggleState, ToggleType};

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
    /// D-Bus menu path for this item (e.g. "/MenuBar").
    pub menu_path: Option<String>,
    /// The menu tree, if available.
    pub menu: Option<Vec<MenuItem>>,
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
    /// Activate the tray item at the given address (default click).
    Activate(String),
    /// Activate a specific menu item by submenu ID.
    MenuItemActivate {
        address: String,
        menu_path: String,
        submenu_id: i32,
    },
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
        let item_states: TrayItemStates = Rc::new(RefCell::new(HashMap::new()));
        glib::spawn_future_local(async move {
            while let Ok(update) = rx.recv().await {
                match update {
                    TrayUpdate::AddOrUpdate(info) => {
                        add_or_update_tray_item(&container_clone, &info, &cmd_tx, &item_states);
                    }
                    TrayUpdate::Remove(address) => {
                        item_states.borrow_mut().remove(&address);
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
        for (address, (item, menu)) in items.iter() {
            let info = tray_item_info(address, item, menu.as_ref());
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
                    Ok(TrayCommand::MenuItemActivate { address, menu_path, submenu_id }) => {
                        let req = system_tray::client::ActivateRequest::MenuItem {
                            address: address.clone(),
                            menu_path,
                            submenu_id,
                        };
                        if let Err(e) = client.activate(req).await {
                            tracing::warn!("tray menu activate failed for {address}: {e}");
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
            // No menu yet on initial Add; it arrives via UpdateEvent::Menu.
            let info = tray_item_info(&address, &item, None);
            let _ = tx.send(TrayUpdate::AddOrUpdate(info)).await;
        }
        system_tray::client::Event::Update(address, _update) => {
            let info = {
                let items = client.items();
                let items = items.lock().unwrap();
                items
                    .get(&address)
                    .map(|(item, menu)| tray_item_info(&address, item, menu.as_ref()))
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

/// Convert a StatusNotifierItem (and optional menu) to our simplified TrayItemInfo.
fn tray_item_info(
    address: &str,
    item: &system_tray::item::StatusNotifierItem,
    menu: Option<&system_tray::menu::TrayMenu>,
) -> TrayItemInfo {
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
        menu_path: item.menu.clone(),
        menu: menu.map(|m| m.submenus.clone()),
    }
}

/// Per-tray-item state stored on the GTK side.
struct TrayItemState {
    menu_items: Vec<MenuItem>,
    menu_path: Option<String>,
}

type TrayItemStates = Rc<RefCell<HashMap<String, Rc<RefCell<TrayItemState>>>>>;

/// Add or update a tray item button in the container.
fn add_or_update_tray_item(
    container: &gtk4::Box,
    info: &TrayItemInfo,
    cmd_tx: &async_channel::Sender<TrayCommand>,
    item_states: &TrayItemStates,
) {
    // Store/update menu state for this item.
    // Done before the existing-item check so menu updates are never dropped.
    let state = if let Some(existing) = item_states.borrow().get(&info.address) {
        let mut st = existing.borrow_mut();
        st.menu_items = info.menu.clone().unwrap_or_default();
        st.menu_path = info.menu_path.clone();
        existing.clone()
    } else {
        let state = Rc::new(RefCell::new(TrayItemState {
            menu_items: info.menu.clone().unwrap_or_default(),
            menu_path: info.menu_path.clone(),
        }));
        item_states
            .borrow_mut()
            .insert(info.address.clone(), state.clone());
        state
    };

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

    // Click: show menu popover if menu is available, otherwise default activate.
    let cmd_tx = cmd_tx.clone();
    let address = info.address.clone();
    btn.connect_clicked(move |btn| {
        let st = state.borrow();
        if st.menu_items.is_empty() || st.menu_path.is_none() {
            // No menu — fall back to default activation.
            let _ = cmd_tx.try_send(TrayCommand::Activate(address.clone()));
            return;
        }
        show_tray_menu(btn, &st.menu_items, &address, st.menu_path.as_deref().unwrap(), &cmd_tx);
    });

    container.append(&btn);
}

/// Build and show a popover menu for a tray item.
fn show_tray_menu(
    btn: &gtk4::Button,
    items: &[MenuItem],
    address: &str,
    menu_path: &str,
    cmd_tx: &async_channel::Sender<TrayCommand>,
) {
    let menu_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    menu_box.add_css_class("psh-bar-tray-menu");

    build_menu_level(&menu_box, items, address, menu_path, cmd_tx);

    let popover = gtk4::Popover::new();
    popover.set_child(Some(&menu_box));
    popover.set_parent(btn);
    popover.set_has_arrow(false);
    popover.connect_closed(|p| p.unparent());
    popover.popup();
}

/// Recursively build menu widgets for one level of menu items.
fn build_menu_level(
    container: &gtk4::Box,
    items: &[MenuItem],
    address: &str,
    menu_path: &str,
    cmd_tx: &async_channel::Sender<TrayCommand>,
) {
    for item in items {
        if !item.visible {
            continue;
        }

        if item.menu_type == MenuType::Separator {
            let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
            container.append(&sep);
            continue;
        }

        // Items with children_display == "submenu" get a nested submenu.
        if item.children_display.as_deref() == Some("submenu") && !item.submenu.is_empty() {
            let label_text = item.label.as_deref().unwrap_or("...");
            let expander = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

            let header_btn = gtk4::Button::with_label(&format!("{label_text}  ▸"));
            header_btn.add_css_class("psh-bar-tray-menu-item");
            header_btn.set_halign(gtk4::Align::Fill);
            if !item.enabled {
                header_btn.set_sensitive(false);
            }

            let sub_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            sub_box.set_margin_start(12);
            sub_box.set_visible(false);
            build_menu_level(&sub_box, &item.submenu, address, menu_path, cmd_tx);

            let sub_box_ref = sub_box.clone();
            header_btn.connect_clicked(move |_| {
                sub_box_ref.set_visible(!sub_box_ref.is_visible());
            });

            expander.append(&header_btn);
            expander.append(&sub_box);
            container.append(&expander);
            continue;
        }

        let label_text = item.label.as_deref().unwrap_or("");
        if label_text.is_empty() && item.submenu.is_empty() {
            continue;
        }

        // Build the label with optional toggle indicator.
        let display_label = match item.toggle_type {
            ToggleType::Checkmark => {
                let check = if item.toggle_state == ToggleState::On { "✓ " } else { "   " };
                format!("{check}{label_text}")
            }
            ToggleType::Radio => {
                let radio = if item.toggle_state == ToggleState::On { "● " } else { "○ " };
                format!("{radio}{label_text}")
            }
            ToggleType::CannotBeToggled => label_text.to_string(),
        };

        let menu_btn = gtk4::Button::with_label(&display_label);
        menu_btn.add_css_class("psh-bar-tray-menu-item");
        menu_btn.set_halign(gtk4::Align::Fill);
        if let Some(label) = menu_btn.child().and_then(|c| c.downcast::<gtk4::Label>().ok()) {
            label.set_halign(gtk4::Align::Start);
        }

        if !item.enabled {
            menu_btn.set_sensitive(false);
        }

        // If this item has submenu children (without children_display hint), show inline.
        let has_inline_submenu = !item.submenu.is_empty();

        let cmd_tx_click = cmd_tx.clone();
        let address_click = address.to_string();
        let menu_path_click = menu_path.to_string();
        let submenu_id = item.id;
        menu_btn.connect_clicked(move |btn| {
            let _ = cmd_tx_click.try_send(TrayCommand::MenuItemActivate {
                address: address_click.clone(),
                menu_path: menu_path_click.clone(),
                submenu_id,
            });
            // Close the popover.
            if let Some(popover) = btn
                .ancestor(gtk4::Popover::static_type())
                .and_then(|w| w.downcast::<gtk4::Popover>().ok())
            {
                popover.popdown();
            }
        });

        container.append(&menu_btn);

        if has_inline_submenu {
            let sub_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            sub_box.set_margin_start(12);
            build_menu_level(&sub_box, &item.submenu, address, menu_path, cmd_tx);
            container.append(&sub_box);
        }
    }
}

/// Set a pixmap icon on a tray button from ARGB32 data.
fn set_pixmap_icon(btn: &gtk4::Button, info: &TrayItemInfo) {
    if let Some((width, height, pixels)) = info
        .icon_pixmap
        .as_ref()
        .filter(|(w, h, p)| *w > 0 && *h > 0 && p.len() >= (*w as usize) * (*h as usize) * 4)
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
