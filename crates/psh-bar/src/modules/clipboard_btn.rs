//! Clipboard button module — triggers the clipboard history picker.

use psh_core::ipc::Message;

use super::ipc_button::IpcButtonModule;

/// A button that sends [`Message::ShowClipboardHistory`] to open the clipboard picker.
pub struct ClipboardButtonModule;

impl ClipboardButtonModule {
    /// Create as a generic IPC button.
    pub fn into_ipc_button() -> IpcButtonModule {
        IpcButtonModule {
            module_name: "clipboard",
            label: "\u{f328}",
            css_class: "psh-bar-clipboard-btn",
            tooltip: "Clipboard history",
            message: Message::ShowClipboardHistory,
        }
    }
}
