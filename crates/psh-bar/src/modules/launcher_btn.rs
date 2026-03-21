//! Launcher button module — triggers the app launcher overlay.

use psh_core::ipc::Message;

use super::ipc_button::IpcButtonModule;

/// A button that sends [`Message::ToggleLauncher`] to toggle the app launcher.
pub struct LauncherButtonModule;

impl LauncherButtonModule {
    /// Create as a generic IPC button.
    pub fn into_ipc_button() -> IpcButtonModule {
        IpcButtonModule {
            module_name: "launcher",
            label: "\u{f002}",
            css_class: "psh-bar-launcher-btn",
            tooltip: "Open launcher",
            message: Message::ToggleLauncher,
        }
    }
}
