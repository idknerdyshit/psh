//! Generic IPC button module — a button that sends an IPC message on click.

use gtk4::glib;
use gtk4::prelude::*;
use psh_core::ipc::Message;

use super::{BarModule, ModuleContext};

/// A generic bar button that sends an IPC message when clicked.
///
/// Used by [`super::launcher_btn::LauncherButtonModule`] and
/// [`super::clipboard_btn::ClipboardButtonModule`] to avoid duplication.
pub struct IpcButtonModule {
    pub module_name: &'static str,
    pub label: &'static str,
    pub css_class: &'static str,
    pub tooltip: &'static str,
    pub message: Message,
}

impl BarModule for IpcButtonModule {
    fn name(&self) -> &'static str {
        self.module_name
    }

    fn widget(&self, ctx: &ModuleContext) -> gtk4::Widget {
        let btn = gtk4::Button::with_label(self.label);
        btn.add_css_class(self.css_class);
        btn.set_tooltip_text(Some(self.tooltip));

        let tx = ctx.ipc_tx.clone();
        let msg = self.message.clone();
        btn.connect_clicked(move |_| {
            let tx = tx.clone();
            let msg = msg.clone();
            glib::spawn_future_local(async move {
                if let Err(e) = tx.send(msg).await {
                    tracing::warn!("failed to send IPC message: {e}");
                }
            });
        });

        btn.upcast()
    }
}
