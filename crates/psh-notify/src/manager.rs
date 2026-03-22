use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use async_channel::Sender;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;
use psh_core::config::NotifyConfig;

use crate::dbus_server::{DbusToGtk, GtkToDbus, ImageData, Notification, Urgency};

/// Sanitize notification body text for safe Pango markup rendering.
///
/// The fd.o notification spec allows a limited subset of HTML-like tags:
/// `<b>`, `<i>`, `<u>`, and `</a>`. All other markup is escaped.
/// `<a href="...">` tags are not restored due to variable content — link text
/// still renders, just without the hyperlink.
fn sanitize_markup(input: &str) -> String {
    glib::markup_escape_text(input)
        .replace("&lt;b&gt;", "<b>")
        .replace("&lt;/b&gt;", "</b>")
        .replace("&lt;i&gt;", "<i>")
        .replace("&lt;/i&gt;", "</i>")
        .replace("&lt;u&gt;", "<u>")
        .replace("&lt;/u&gt;", "</u>")
        .replace("&lt;/a&gt;", "</a>")
}

struct ActiveNotification {
    id: u32,
    content_box: gtk4::Box,
    timeout_source: Option<glib::SourceId>,
}

pub struct NotificationManager {
    notifications: Vec<ActiveNotification>,
    config: NotifyConfig,
    window: gtk4::ApplicationWindow,
    stack: gtk4::Box,
    signal_tx: Sender<GtkToDbus>,
    ipc_count_tx: Sender<u32>,
}

pub type ManagerRef = Rc<RefCell<NotificationManager>>;

impl NotificationManager {
    pub fn new(
        app: gtk4::Application,
        config: NotifyConfig,
        signal_tx: Sender<GtkToDbus>,
        ipc_count_tx: Sender<u32>,
    ) -> ManagerRef {
        let window = gtk4::ApplicationWindow::builder()
            .application(&app)
            .default_width(config.width as i32)
            .decorated(false)
            .build();

        window.init_layer_shell();
        window.set_layer(gtk4_layer_shell::Layer::Overlay);
        window.set_anchor(gtk4_layer_shell::Edge::Top, true);
        window.set_anchor(gtk4_layer_shell::Edge::Right, true);
        window.set_margin(gtk4_layer_shell::Edge::Right, 10);
        window.set_margin(gtk4_layer_shell::Edge::Top, 10);
        window.set_namespace(Some("psh-notify"));

        let stack = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        stack.add_css_class("psh-notify-stack");
        window.set_child(Some(&stack));

        // Start hidden — shown when first notification arrives
        window.set_visible(false);
        window.present();

        Rc::new(RefCell::new(Self {
            notifications: Vec::new(),
            config,
            window,
            stack,
            signal_tx,
            ipc_count_tx,
        }))
    }

    /// Update the runtime config (called on config hot-reload).
    pub fn update_config(manager: &ManagerRef, config: NotifyConfig) {
        manager.borrow_mut().config = config;
    }

    /// Handle an incoming D-Bus message on the GTK thread.
    pub fn handle(manager: &ManagerRef, msg: DbusToGtk) {
        match msg {
            DbusToGtk::Notify(notif) => Self::show_or_replace(manager, notif),
            DbusToGtk::Close(id) => {
                manager.borrow_mut().dismiss(id, 3); // reason 3 = closed by CloseNotification
            }
        }
    }

    fn show_or_replace(manager: &ManagerRef, notif: Notification) {
        // Check for replace-id — narrow borrow to find existing notification
        let replacing = {
            let inner = manager.borrow();
            if notif.replaces_id > 0 {
                inner
                    .notifications
                    .iter()
                    .position(|n| n.id == notif.replaces_id)
            } else {
                None
            }
        };

        if let Some(idx) = replacing {
            // Extract what we need from the manager, then drop the borrow
            let (content_box, config, signal_tx) = {
                let mut inner = manager.borrow_mut();
                let active = &mut inner.notifications[idx];

                // Clear old timeout
                if let Some(source) = active.timeout_source.take() {
                    source.remove();
                }

                Self::clear_content(&active.content_box);
                (
                    active.content_box.clone(),
                    inner.config.clone(),
                    inner.signal_tx.clone(),
                )
            };

            // Populate without holding a borrow
            Self::populate_content(
                &content_box,
                &notif,
                &config,
                &signal_tx,
                Rc::clone(manager),
            );

            // Set new timeout — narrow borrow
            let timeout_source = if notif.urgency != Urgency::Critical {
                let timeout_ms = manager.borrow().effective_timeout(&notif);
                if let Some(timeout_ms) = timeout_ms {
                    let id = notif.id;
                    let mgr = Rc::clone(manager);
                    Some(glib::timeout_add_local_once(
                        Duration::from_millis(timeout_ms),
                        move || {
                            mgr.borrow_mut().dismiss(id, 1);
                        },
                    ))
                } else {
                    None
                }
            } else {
                None
            };
            manager.borrow_mut().notifications[idx].timeout_source = timeout_source;
            return;
        }

        // Enforce max_visible by dismissing oldest
        loop {
            let oldest_id = {
                let inner = manager.borrow();
                if inner.notifications.len() >= inner.config.max_visible as usize {
                    inner.notifications.first().map(|n| n.id)
                } else {
                    None
                }
            };
            if let Some(id) = oldest_id {
                manager.borrow_mut().dismiss(id, 1); // reason 1 = expired
            } else {
                break;
            }
        }

        // Extract config and sender for building content
        let (config, signal_tx, stack) = {
            let inner = manager.borrow();
            (
                inner.config.clone(),
                inner.signal_tx.clone(),
                inner.stack.clone(),
            )
        };

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        content_box.add_css_class("psh-notify-popup");
        Self::add_urgency_class(&content_box, notif.urgency);
        content_box.set_margin_top(12);
        content_box.set_margin_bottom(12);
        content_box.set_margin_start(16);
        content_box.set_margin_end(16);

        let has_default_action = notif.actions.iter().any(|(k, _)| k == "default");

        Self::populate_content(
            &content_box,
            &notif,
            &config,
            &signal_tx,
            Rc::clone(manager),
        );

        // Click on body to invoke default action
        if has_default_action {
            let gesture = gtk4::GestureClick::new();
            let id = notif.id;
            let signal_tx = signal_tx.clone();
            let mgr = Rc::clone(manager);
            gesture.connect_released(move |_, _, _, _| {
                if let Err(e) = signal_tx.try_send(GtkToDbus::ActionInvoked {
                    id,
                    action_key: "default".into(),
                }) {
                    tracing::warn!("failed to send ActionInvoked: {e}");
                }
                mgr.borrow_mut().dismiss(id, 2);
            });
            content_box.add_controller(gesture);
        }

        // Append to the stack
        stack.append(&content_box);

        // Set up timeout (not for critical, not for expire_timeout=0)
        let timeout_source = if notif.urgency != Urgency::Critical {
            let timeout_ms = manager.borrow().effective_timeout(&notif);
            if let Some(timeout_ms) = timeout_ms {
                let id = notif.id;
                let mgr = Rc::clone(manager);
                Some(glib::timeout_add_local_once(
                    Duration::from_millis(timeout_ms),
                    move || {
                        mgr.borrow_mut().dismiss(id, 1);
                    },
                ))
            } else {
                None
            }
        } else {
            None
        };

        let mut inner = manager.borrow_mut();
        inner.notifications.push(ActiveNotification {
            id: notif.id,
            content_box,
            timeout_source,
        });

        inner.update_visibility();
        inner.send_count();
    }

    pub fn dismiss(&mut self, id: u32, reason: u32) {
        if let Some(idx) = self.notifications.iter().position(|n| n.id == id) {
            let active = self.notifications.remove(idx);
            if let Some(source) = active.timeout_source {
                source.remove();
            }
            self.stack.remove(&active.content_box);

            if let Err(e) = self.signal_tx.try_send(GtkToDbus::Closed { id, reason }) {
                tracing::warn!("failed to send NotificationClosed: {e}");
            }

            self.update_visibility();
            self.send_count();
        }
    }

    /// Show the overlay window when notifications exist, hide when empty.
    fn update_visibility(&self) {
        self.window.set_visible(!self.notifications.is_empty());
    }

    fn send_count(&self) {
        let count = self.notifications.len() as u32;
        if let Err(e) = self.ipc_count_tx.try_send(count) {
            tracing::warn!("failed to send notification count: {e}");
        }
    }

    /// Compute the effective timeout for a notification.
    ///
    /// Per fd.o spec: -1 means server default, 0 means never expire,
    /// positive values are milliseconds.
    fn effective_timeout(&self, notif: &Notification) -> Option<u64> {
        match notif.expire_timeout {
            0 => None,
            t if t > 0 => Some(t as u64),
            _ => Some(self.config.default_timeout_ms),
        }
    }

    fn add_urgency_class(container: &gtk4::Box, urgency: Urgency) {
        match urgency {
            Urgency::Low => container.add_css_class("psh-notify-urgency-low"),
            Urgency::Normal => container.add_css_class("psh-notify-urgency-normal"),
            Urgency::Critical => container.add_css_class("psh-notify-urgency-critical"),
        }
    }

    fn clear_content(container: &gtk4::Box) {
        while let Some(child) = container.first_child() {
            container.remove(&child);
        }
    }

    fn populate_content(
        container: &gtk4::Box,
        notif: &Notification,
        config: &NotifyConfig,
        signal_tx: &Sender<GtkToDbus>,
        manager: ManagerRef,
    ) {
        // Header row: [icon] summary [close button]
        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        header.add_css_class("psh-notify-header");

        // Icon
        if let Some(image) = Self::build_icon(notif, config) {
            header.append(&image);
        }

        // Summary
        let summary = gtk4::Label::new(Some(&notif.summary));
        summary.add_css_class("psh-notify-summary");
        summary.set_halign(gtk4::Align::Start);
        summary.set_hexpand(true);
        summary.set_wrap(true);
        header.append(&summary);

        // Close button
        let close_btn = gtk4::Button::from_icon_name("window-close-symbolic");
        close_btn.add_css_class("psh-notify-close");
        close_btn.set_valign(gtk4::Align::Start);
        let id = notif.id;
        let mgr = Rc::clone(&manager);
        close_btn.connect_clicked(move |_| {
            mgr.borrow_mut().dismiss(id, 2); // reason 2 = dismissed by user
        });
        header.append(&close_btn);

        container.append(&header);

        // Body
        if !notif.body.is_empty() {
            let body = gtk4::Label::new(None);
            body.add_css_class("psh-notify-body");
            body.set_halign(gtk4::Align::Start);
            body.set_wrap(true);
            body.set_markup(&sanitize_markup(&notif.body));
            container.append(&body);
        }

        // Action buttons (skip "default" action — it's handled by body click)
        let non_default_actions: Vec<_> = notif
            .actions
            .iter()
            .filter(|(k, _)| k != "default")
            .collect();
        if !non_default_actions.is_empty() {
            let actions_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            actions_box.add_css_class("psh-notify-actions");

            for (key, label) in &non_default_actions {
                let btn = gtk4::Button::with_label(label);
                btn.add_css_class("psh-notify-action");
                let id = notif.id;
                let key = key.to_string();
                let tx = signal_tx.clone();
                let mgr = Rc::clone(&manager);
                btn.connect_clicked(move |_| {
                    if let Err(e) = tx.try_send(GtkToDbus::ActionInvoked {
                        id,
                        action_key: key.clone(),
                    }) {
                        tracing::warn!("failed to send ActionInvoked: {e}");
                    }
                    mgr.borrow_mut().dismiss(id, 2);
                });
                actions_box.append(&btn);
            }

            container.append(&actions_box);
        }
    }

    fn build_icon(notif: &Notification, config: &NotifyConfig) -> Option<gtk4::Image> {
        let pixel_size = config.icon_size as i32;

        // Priority: image-data hint > app_icon parameter
        if let Some(ref img_data) = notif.image_data
            && let Some(texture) = Self::image_data_to_texture(img_data)
        {
            let image = gtk4::Image::from_paintable(Some(&texture));
            image.set_pixel_size(pixel_size);
            image.add_css_class("psh-notify-icon");
            return Some(image);
        }

        if !notif.app_icon.is_empty() {
            let image = gtk4::Image::from_icon_name(&notif.app_icon);
            image.set_pixel_size(pixel_size);
            image.add_css_class("psh-notify-icon");
            return Some(image);
        }

        None
    }

    fn image_data_to_texture(img: &ImageData) -> Option<gdk::Texture> {
        if img.width <= 0 || img.height <= 0 || img.rowstride <= 0 || img.data.is_empty() {
            return None;
        }

        let expected = (img.height as usize).saturating_mul(img.rowstride as usize);
        if img.data.len() < expected {
            tracing::warn!(
                "image-data too small: have {} bytes, need {} ({}x{}, stride {})",
                img.data.len(),
                expected,
                img.width,
                img.height,
                img.rowstride,
            );
            return None;
        }

        let format = if img.has_alpha {
            gdk::MemoryFormat::R8g8b8a8
        } else {
            gdk::MemoryFormat::R8g8b8
        };

        let bytes = glib::Bytes::from(&img.data);
        let texture = gdk::MemoryTexture::new(
            img.width,
            img.height,
            format,
            &bytes,
            img.rowstride as usize,
        );
        Some(texture.into())
    }
}
