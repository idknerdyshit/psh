use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::debug;

use crate::{PshError, Result};

/// IPC message types exchanged between psh components.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Message {
    Ping,
    Pong,
    ConfigReloaded,
    ToggleLauncher,
    ShowClipboardHistory,
    LockScreen,
    NotificationCount { count: u32 },
    SetWallpaper { path: String },
}

/// Returns the IPC socket path at `$XDG_RUNTIME_DIR/psh.sock`.
pub fn socket_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(|dir| PathBuf::from(dir).join("psh.sock"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/psh.sock"))
}

/// Write a length-prefixed JSON message to any async writer.
pub async fn send_to<W: AsyncWriteExt + Unpin>(writer: &mut W, msg: &Message) -> Result<()> {
    let payload = serde_json::to_vec(msg)?;
    let len = (payload.len() as u32).to_be_bytes();
    writer
        .write_all(&len)
        .await
        .map_err(|e| PshError::Ipc(e.to_string()))?;
    writer
        .write_all(&payload)
        .await
        .map_err(|e| PshError::Ipc(e.to_string()))?;
    debug!("sent ipc message: {msg:?}");
    Ok(())
}

/// Read a length-prefixed JSON message from any async reader.
pub async fn recv_from<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Message> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| PshError::Ipc(e.to_string()))?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > 1024 * 1024 {
        return Err(PshError::Ipc(format!("message too large: {len} bytes")));
    }

    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .await
        .map_err(|e| PshError::Ipc(e.to_string()))?;

    let msg: Message = serde_json::from_slice(&buf)?;
    debug!("received ipc message: {msg:?}");
    Ok(msg)
}

/// Write a length-prefixed JSON message to a UnixStream.
pub async fn send(stream: &mut UnixStream, msg: &Message) -> Result<()> {
    send_to(stream, msg).await
}

/// Read a length-prefixed JSON message from a UnixStream.
pub async fn recv(stream: &mut UnixStream) -> Result<Message> {
    recv_from(stream).await
}

/// Connect to the psh IPC hub as a client.
pub async fn connect() -> Result<UnixStream> {
    let path = socket_path();
    UnixStream::connect(&path)
        .await
        .map_err(|e| PshError::Ipc(format!("failed to connect to {}: {e}", path.display())))
}

/// Bind the IPC socket as the hub (psh-bar).
pub async fn bind() -> Result<UnixListener> {
    let path = socket_path();
    // Remove stale socket if it exists
    let _ = std::fs::remove_file(&path);
    UnixListener::bind(&path)
        .map_err(|e| PshError::Ipc(format!("failed to bind {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_roundtrip() {
        let msg = Message::NotificationCount { count: 42 };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: Message = serde_json::from_str(&json).unwrap();
        match decoded {
            Message::NotificationCount { count } => assert_eq!(count, 42),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn all_variants_serialize() {
        let messages = vec![
            Message::Ping,
            Message::Pong,
            Message::ConfigReloaded,
            Message::ToggleLauncher,
            Message::ShowClipboardHistory,
            Message::LockScreen,
            Message::NotificationCount { count: 0 },
            Message::SetWallpaper {
                path: "/test.png".into(),
            },
        ];
        for msg in messages {
            let json = serde_json::to_string(&msg).unwrap();
            let _: Message = serde_json::from_str(&json).unwrap();
        }
    }
}
