//! Bar module trait, context, and registry.
//!
//! Every bar module implements [`BarModule`] and is instantiated via [`create_module`].
//! Modules receive a [`ModuleContext`] providing IPC channels and configuration.

pub mod battery;
pub mod claude;
pub mod clipboard_btn;
pub mod clock;
pub mod ipc_button;
pub mod launcher_btn;
pub mod network;
pub mod notifications;
pub mod tray;
pub mod volume;
pub mod window_title;
pub mod workspaces;

use psh_core::config::BarConfig;
use psh_core::ipc::Message;

/// Context provided to every module during widget construction.
///
/// Each module gets its own `ipc_rx` receiver so the IPC hub can fan out
/// incoming messages to all modules independently. The `ipc_tx` sender is
/// shared (cloned) across all modules and delivers messages from modules
/// back to the IPC hub for broadcast to connected clients.
pub struct ModuleContext {
    /// Sender for messages from this module to the IPC hub (click actions, etc.).
    pub ipc_tx: async_channel::Sender<Message>,
    /// Receiver for messages from the IPC hub to this module (per-module channel).
    pub ipc_rx: async_channel::Receiver<Message>,
    /// Bar configuration.
    pub config: BarConfig,
    /// Handle to the shared tokio runtime for spawning async backend tasks.
    pub rt: tokio::runtime::Handle,
}

/// Trait implemented by every bar module.
///
/// The `widget` method is called once on the GTK main thread during bar
/// construction. Modules that need asynchronous data (D-Bus, sockets, etc.)
/// should spawn background tasks inside `widget()` using
/// `glib::spawn_future_local` and `async_channel`.
pub trait BarModule {
    /// Unique name used in config arrays and CSS class naming.
    fn name(&self) -> &'static str;

    /// Build the GTK widget for this module.
    ///
    /// Called once during bar startup on the GTK main thread.
    fn widget(&self, ctx: &ModuleContext) -> gtk4::Widget;
}

/// Default module list for the left section when config is empty.
pub const DEFAULT_LEFT: &[&str] = &["workspaces", "window_title"];

/// Default module list for the center section when config is empty.
pub const DEFAULT_CENTER: &[&str] = &["clock"];

/// Default module list for the right section when config is empty.
pub const DEFAULT_RIGHT: &[&str] = &["volume", "network", "battery", "tray"];

/// All known module names (used by registry consistency tests).
#[cfg(test)]
pub const KNOWN_MODULES: &[&str] = &[
    "claude",
    "clock",
    "battery",
    "workspaces",
    "window_title",
    "volume",
    "network",
    "tray",
    "launcher",
    "clipboard",
    "notifications",
];

/// Create a bar module by name.
///
/// Returns `None` if the name is not a known module, logging a warning.
pub fn create_module(name: &str) -> Option<Box<dyn BarModule>> {
    match name {
        "claude" => Some(Box::new(claude::ClaudeModule)),
        "clock" => Some(Box::new(clock::ClockModule)),
        "battery" => Some(Box::new(battery::BatteryModule)),
        "workspaces" => Some(Box::new(workspaces::WorkspacesModule)),
        "window_title" => Some(Box::new(window_title::WindowTitleModule)),
        "volume" => Some(Box::new(volume::VolumeModule)),
        "network" => Some(Box::new(network::NetworkModule)),
        "tray" => Some(Box::new(tray::TrayModule)),
        "launcher" => Some(Box::new(
            launcher_btn::LauncherButtonModule::into_ipc_button(),
        )),
        "clipboard" => Some(Box::new(
            clipboard_btn::ClipboardButtonModule::into_ipc_button(),
        )),
        "notifications" => Some(Box::new(notifications::NotificationsModule)),
        _ => {
            tracing::warn!("unknown bar module: {name}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_known_modules() {
        for name in KNOWN_MODULES {
            assert!(
                create_module(name).is_some(),
                "create_module({name:?}) returned None"
            );
        }
    }

    #[test]
    fn create_unknown_module_returns_none() {
        assert!(create_module("nonexistent").is_none());
        assert!(create_module("").is_none());
    }

    #[test]
    fn default_lists_contain_valid_modules() {
        for name in DEFAULT_LEFT
            .iter()
            .chain(DEFAULT_CENTER)
            .chain(DEFAULT_RIGHT)
        {
            assert!(
                KNOWN_MODULES.contains(name),
                "default module {name:?} not in KNOWN_MODULES"
            );
        }
    }

    #[test]
    fn module_names_match_registry() {
        for name in KNOWN_MODULES {
            let module = create_module(name).unwrap();
            assert_eq!(
                module.name(),
                *name,
                "module registered as {name:?} reports name {:?}",
                module.name()
            );
        }
    }
}
