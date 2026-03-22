pub mod config;
pub mod dbus;
pub mod error;
pub mod ipc;
pub mod logging;
pub mod palette;
#[cfg(feature = "gtk")]
pub mod theme;

pub use error::{PshError, Result};
