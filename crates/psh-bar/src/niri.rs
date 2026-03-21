//! Niri IPC helpers — shared connection and request/response handling.
//!
//! Niri exposes a JSON IPC socket at `$NIRI_SOCKET`. This module provides
//! async helpers for sending requests and subscribing to the event stream.

use std::path::PathBuf;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use psh_core::Result;

/// Maximum number of connection retries when niri socket is not yet available.
const MAX_RETRIES: u32 = 15;

/// Delay between connection retries.
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

/// Returns the niri IPC socket path from `$NIRI_SOCKET`.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var("NIRI_SOCKET").ok().map(PathBuf::from)
}

/// Returns true if niri IPC is available (i.e., `$NIRI_SOCKET` is set).
pub fn is_available() -> bool {
    socket_path().is_some()
}

/// Connect to the niri IPC socket with retries.
///
/// Retries up to [`MAX_RETRIES`] times with a 2-second delay if the socket
/// is not yet available (e.g., bar starts before niri).
pub async fn connect() -> Result<UnixStream> {
    let path = socket_path().ok_or_else(|| {
        psh_core::PshError::Ipc("NIRI_SOCKET not set".into())
    })?;

    for attempt in 0..MAX_RETRIES {
        match UnixStream::connect(&path).await {
            Ok(stream) => {
                tracing::debug!("connected to niri socket");
                return Ok(stream);
            }
            Err(e) if attempt < MAX_RETRIES - 1 => {
                tracing::debug!(
                    "niri socket not ready (attempt {}/{}): {e}",
                    attempt + 1,
                    MAX_RETRIES
                );
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Err(e) => {
                return Err(psh_core::PshError::Ipc(format!(
                    "failed to connect to niri after {MAX_RETRIES} attempts: {e}"
                )));
            }
        }
    }

    unreachable!()
}

/// Send a request to niri and read the response.
///
/// Opens a new connection for each request (niri expects one request per connection
/// for non-event-stream requests).
pub async fn request(req: &niri_ipc::Request) -> Result<niri_ipc::Response> {
    let mut stream = connect().await?;
    let json = serde_json::to_string(req)
        .map_err(|e| psh_core::PshError::Ipc(e.to_string()))?;

    stream
        .write_all(json.as_bytes())
        .await
        .map_err(|e| psh_core::PshError::Ipc(e.to_string()))?;
    stream
        .shutdown()
        .await
        .map_err(|e| psh_core::PshError::Ipc(e.to_string()))?;

    let mut buf = String::new();
    let mut reader = BufReader::new(stream);
    reader
        .read_line(&mut buf)
        .await
        .map_err(|e| psh_core::PshError::Ipc(e.to_string()))?;

    let response: niri_ipc::Response = serde_json::from_str(&buf)
        .map_err(|e| psh_core::PshError::Ipc(format!("bad niri response: {e}")))?;

    Ok(response)
}

/// Subscribe to niri's event stream.
///
/// Returns a `BufReader` over the connected stream. Each line is a JSON-encoded
/// [`niri_ipc::Event`]. The caller should read lines in a loop.
pub async fn event_stream() -> Result<BufReader<UnixStream>> {
    let mut stream = connect().await?;
    let json = serde_json::to_string(&niri_ipc::Request::EventStream)
        .map_err(|e| psh_core::PshError::Ipc(e.to_string()))?;

    stream
        .write_all(json.as_bytes())
        .await
        .map_err(|e| psh_core::PshError::Ipc(e.to_string()))?;

    // Do NOT shutdown the write half here — niri keeps the connection open
    // for the event stream and we need to keep reading from it.

    // Read the initial Ok response line
    let mut reader = BufReader::new(stream);
    let mut first_line = String::new();
    reader
        .read_line(&mut first_line)
        .await
        .map_err(|e| psh_core::PshError::Ipc(e.to_string()))?;

    // Verify it's an Ok response
    let resp: Value = serde_json::from_str(&first_line)
        .map_err(|e| psh_core::PshError::Ipc(format!("bad event stream response: {e}")))?;

    if resp.get("Ok").is_none() {
        return Err(psh_core::PshError::Ipc(format!(
            "unexpected event stream response: {first_line}"
        )));
    }

    Ok(reader)
}

/// Parse a JSON line from the event stream into a niri Event.
pub fn parse_event(line: &str) -> Result<niri_ipc::Event> {
    serde_json::from_str(line)
        .map_err(|e| psh_core::PshError::Ipc(format!("bad niri event: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_from_env() {
        // When NIRI_SOCKET is not set (typical in test), should return None
        // We can't reliably test the Some case without setting the env var
        // which would affect other tests. Just verify the function doesn't panic.
        let _ = socket_path();
    }

    #[test]
    fn is_available_reflects_env() {
        // This just exercises the function — actual value depends on env
        let _ = is_available();
    }

    #[test]
    fn parse_event_valid_workspace_event() {
        // Build a valid event via the niri_ipc types and round-trip it
        let ws = niri_ipc::Workspace {
            id: 1,
            idx: 1,
            name: None,
            output: Some("DP-1".into()),
            is_active: true,
            is_focused: true,
            is_urgent: false,
            active_window_id: None,
        };
        let event = niri_ipc::Event::WorkspacesChanged {
            workspaces: vec![ws],
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed = parse_event(&json);
        assert!(parsed.is_ok(), "should parse valid workspace event: {json}");
    }

    #[test]
    fn parse_event_invalid_json() {
        let result = parse_event("not json");
        assert!(result.is_err());
    }

    #[test]
    fn parse_event_empty_string() {
        let result = parse_event("");
        assert!(result.is_err());
    }
}
