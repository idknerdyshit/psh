use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, PshError>;

#[derive(Debug, thiserror::Error)]
pub enum PshError {
    #[error("config error: {0}")]
    Config(String),

    #[error("failed to parse config at {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[error("ipc error: {0}")]
    Ipc(String),

    #[error("dbus error: {0}")]
    DBus(#[from] zbus::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),

    #[error("{0}")]
    Other(String),
}
