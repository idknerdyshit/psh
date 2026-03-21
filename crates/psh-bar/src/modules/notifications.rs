//! Notifications module — displays the current notification count.

use gtk4::glib;
use gtk4::prelude::*;
use psh_core::ipc::Message;

use super::{BarModule, ModuleContext};

/// Displays the active notification count, updated via IPC broadcasts
/// from psh-notify.
pub struct NotificationsModule;

impl BarModule for NotificationsModule {
    fn name(&self) -> &'static str {
        "notifications"
    }

    fn widget(&self, ctx: &ModuleContext) -> gtk4::Widget {
        let label = gtk4::Label::new(Some("0"));
        label.add_css_class("psh-bar-notifications");
        label.set_tooltip_text(Some("Notification count"));

        let rx = ctx.ipc_rx.clone();
        let label_clone = label.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                if let Message::NotificationCount { count } = msg {
                    label_clone.set_text(&count.to_string());
                    if count > 0 {
                        label_clone.add_css_class("has-notifications");
                    } else {
                        label_clone.remove_css_class("has-notifications");
                    }
                }
            }
        });

        label.upcast()
    }
}
