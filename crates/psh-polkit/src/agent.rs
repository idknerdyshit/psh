use std::collections::HashMap;
use std::sync::Arc;

use async_channel::{Receiver, Sender};
use secrecy::SecretString;
use zbus::connection::Builder as ConnectionBuilder;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, interface};

use psh_core::Result;

use crate::authority::{self, Identity};

/// Maximum number of authentication attempts before giving up.
const MAX_ATTEMPTS: u32 = 3;

// ---------------------------------------------------------------------------
// Channel message types
// ---------------------------------------------------------------------------

/// Messages sent from the D-Bus agent thread to the GTK thread.
#[derive(Debug)]
pub enum AgentToGtk {
    /// Show the authentication dialog for a new request.
    ShowAuth(AuthRequest),
    /// Authentication failed — show error and allow retry.
    AuthFailed { cookie: String, message: String },
    /// Authentication was cancelled (by system or max retries) — close dialog.
    AuthCancelled { cookie: String },
    /// Authentication succeeded — close dialog.
    AuthSucceeded { cookie: String },
}

/// Authentication request details sent to the GTK thread.
#[derive(Debug)]
pub struct AuthRequest {
    pub cookie: String,
    pub message: String,
    pub icon_name: String,
    pub username: String,
    pub uid: u32,
    pub identity: Identity,
}

/// Response from the GTK thread back to the agent.
#[derive(Debug)]
pub struct AuthResponse {
    pub cookie: String,
    pub password: Option<SecretString>,
}

// ---------------------------------------------------------------------------
// D-Bus interface
// ---------------------------------------------------------------------------

/// Per-session response channels, keyed by cookie.
type SessionMap = Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::mpsc::Sender<AuthResponse>>>>;

/// D-Bus object implementing the `org.freedesktop.PolicyKit1.AuthenticationAgent`
/// interface. Receives authentication requests from polkitd, coordinates with
/// the GTK dialog via channels, and reports results back to the authority.
pub struct PolkitAgent {
    tx: Sender<AgentToGtk>,
    sessions: SessionMap,
    cancel_tx: tokio::sync::broadcast::Sender<String>,
    conn: Arc<Connection>,
}

/// Guard that removes a session from the map when dropped, ensuring cleanup on
/// all exit paths (including panics).
struct SessionGuard {
    cookie: String,
    sessions: SessionMap,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let sessions = self.sessions.clone();
        let cookie = std::mem::take(&mut self.cookie);
        // Try an immediate lock first; if contended, spawn an async cleanup task.
        if let Ok(mut map) = sessions.try_lock() {
            map.remove(&cookie);
        } else if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                sessions.lock().await.remove(&cookie);
            });
        }
    }
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl PolkitAgent {
    /// Called by polkitd when a privileged action needs authentication.
    ///
    /// Returns a D-Bus error on early failures so polkitd doesn't hang waiting
    /// for an `AuthenticationAgentResponse2` that will never come.
    async fn begin_authentication(
        &self,
        action_id: &str,
        message: &str,
        icon_name: &str,
        _details: HashMap<String, String>,
        cookie: &str,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> zbus::fdo::Result<()> {
        tracing::info!("polkit auth request: action={action_id} cookie={cookie}");

        let Some((uid, identity)) = authority::extract_uid(&identities) else {
            tracing::error!("no unix-user identity found in auth request");
            return Err(zbus::fdo::Error::Failed(
                "no unix-user identity found".into(),
            ));
        };

        // Resolve username once and reuse for both display and authentication
        // to avoid TOCTOU if the user database changes between calls.
        let username = authority::uid_to_username(uid).unwrap_or_else(|| format!("uid {uid}"));
        let cookie = cookie.to_string();

        let req = AuthRequest {
            cookie: cookie.clone(),
            message: message.to_string(),
            icon_name: icon_name.to_string(),
            username: username.clone(),
            uid,
            identity: identity.clone(),
        };

        // Show the dialog
        if self.tx.send(AgentToGtk::ShowAuth(req)).await.is_err() {
            tracing::error!("GTK channel closed");
            return Err(zbus::fdo::Error::Failed("agent GTK channel closed".into()));
        }

        // Register a per-session channel so responses are routed correctly even
        // when multiple authentication sessions are active concurrently.
        let (session_tx, mut session_rx) = tokio::sync::mpsc::channel::<AuthResponse>(1);
        self.sessions
            .lock()
            .await
            .insert(cookie.clone(), session_tx);
        let _guard = SessionGuard {
            cookie: cookie.clone(),
            sessions: Arc::clone(&self.sessions),
        };

        // Retry loop — wait for password, authenticate, repeat on failure
        let mut attempts = 0u32;
        loop {
            // Wait for either a user response or a cancellation signal
            let response = tokio::select! {
                resp = session_rx.recv() => {
                    match resp {
                        Some(r) => r,
                        None => {
                            tracing::error!("session channel closed for cookie {cookie}");
                            return Ok(());
                        }
                    }
                }
                _ = self.wait_for_cancel(&cookie) => {
                    tracing::info!("auth cancelled by system for cookie {cookie}");
                    let _ = self.tx.send(AgentToGtk::AuthCancelled {
                        cookie: cookie.clone(),
                    }).await;
                    return Ok(());
                }
            };

            // User clicked Cancel
            let Some(password) = response.password else {
                tracing::info!("auth cancelled by user for cookie {cookie}");
                return Ok(());
            };

            attempts += 1;

            // Try to authenticate via polkit-agent-helper-1.
            // Wrap in select! so system cancellation can interrupt a running auth.
            let auth_result = tokio::select! {
                result = authority::authenticate(&username, &password) => result,
                _ = self.wait_for_cancel(&cookie) => {
                    tracing::info!("auth cancelled by system during authentication for cookie {cookie}");
                    let _ = self.tx.send(AgentToGtk::AuthCancelled {
                        cookie: cookie.clone(),
                    }).await;
                    return Ok(());
                }
            };

            match auth_result {
                Ok(true) => {
                    if let Err(e) = authority::respond(&self.conn, &cookie, &identity).await {
                        tracing::error!("failed to send auth response to polkitd: {e}");
                    }
                    let _ = self.tx.send(AgentToGtk::AuthSucceeded { cookie }).await;
                    return Ok(());
                }
                Ok(false) => {
                    tracing::warn!(
                        "auth failed for cookie {cookie} (attempt {attempts}/{MAX_ATTEMPTS})"
                    );
                    if attempts >= MAX_ATTEMPTS {
                        let _ = self.tx.send(AgentToGtk::AuthCancelled { cookie }).await;
                        return Ok(());
                    }
                    let _ = self
                        .tx
                        .send(AgentToGtk::AuthFailed {
                            cookie: cookie.clone(),
                            message: "Authentication failed. Please try again.".to_string(),
                        })
                        .await;
                }
                Err(e) => {
                    tracing::error!("auth helper error: {e}");
                    let _ = self
                        .tx
                        .send(AgentToGtk::AuthFailed {
                            cookie: cookie.clone(),
                            message: "An internal error occurred. Please try again.".to_string(),
                        })
                        .await;
                    if attempts >= MAX_ATTEMPTS {
                        let _ = self.tx.send(AgentToGtk::AuthCancelled { cookie }).await;
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Called by polkitd when an in-progress authentication should be cancelled.
    async fn cancel_authentication(&self, cookie: &str) {
        tracing::info!("cancel_authentication: cookie={cookie}");
        let _ = self.cancel_tx.send(cookie.to_string());
    }
}

impl PolkitAgent {
    /// Wait until a cancellation is broadcast for our cookie.
    async fn wait_for_cancel(&self, cookie: &str) {
        let mut rx = self.cancel_tx.subscribe();
        loop {
            match rx.recv().await {
                Ok(cancelled_cookie) if cancelled_cookie == cookie => return,
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    // Channel closed — wait forever (dropped when begin_authentication returns)
                    std::future::pending::<()>().await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Start the polkit agent: serve the D-Bus interface and register with the authority.
pub async fn run(
    tx: Sender<AgentToGtk>,
    rx: Receiver<AuthResponse>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> Result<()> {
    let (cancel_tx, _) = tokio::sync::broadcast::channel::<String>(16);
    let sessions: SessionMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    // Dispatcher task: routes incoming GTK responses to per-session channels by cookie.
    let sessions_dispatch = Arc::clone(&sessions);
    tokio::spawn(async move {
        while let Ok(response) = rx.recv().await {
            let session_tx = {
                let map = sessions_dispatch.lock().await;
                map.get(&response.cookie).cloned()
            };
            if let Some(session_tx) = session_tx {
                let _ = session_tx.send(response).await;
            } else {
                tracing::warn!("received response for unknown cookie: {}", response.cookie);
            }
        }
    });

    // Build the system bus connection and serve our agent interface.
    // We need the connection before constructing PolkitAgent (for the Arc), so we
    // build in two steps: first get a plain connection, then serve the interface.
    let conn = ConnectionBuilder::system()?.build().await?;

    let conn = Arc::new(conn);

    let agent = PolkitAgent {
        tx,
        sessions,
        cancel_tx,
        conn: Arc::clone(&conn),
    };

    // Serve the agent interface on the connection's object server
    conn.object_server()
        .at("/org/freedesktop/PolicyKit1/AuthenticationAgent", agent)
        .await?;

    // Register with the polkit authority
    let subject = authority::detect_session_subject()?;
    authority::register(&conn, &subject).await?;

    // Keep running until shutdown is signalled
    tokio::select! {
        _ = std::future::pending::<()>() => {}
        _ = &mut shutdown_rx => {
            tracing::info!("shutting down polkit agent");
        }
    }

    // Unregister on shutdown
    if let Err(e) = authority::unregister(&conn, &subject).await {
        tracing::warn!("failed to unregister from polkit authority: {e}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_guard_removes_entry_on_drop() {
        let sessions: SessionMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let (tx, _rx) = tokio::sync::mpsc::channel::<AuthResponse>(1);
        sessions.lock().await.insert("cookie-1".to_string(), tx);

        {
            let _guard = SessionGuard {
                cookie: "cookie-1".to_string(),
                sessions: Arc::clone(&sessions),
            };
            assert!(sessions.lock().await.contains_key("cookie-1"));
        }
        // Guard dropped — entry should be removed
        assert!(!sessions.lock().await.contains_key("cookie-1"));
    }

    #[tokio::test]
    async fn session_guard_only_removes_own_cookie() {
        let sessions: SessionMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let (tx1, _rx1) = tokio::sync::mpsc::channel::<AuthResponse>(1);
        let (tx2, _rx2) = tokio::sync::mpsc::channel::<AuthResponse>(1);
        sessions.lock().await.insert("cookie-1".to_string(), tx1);
        sessions.lock().await.insert("cookie-2".to_string(), tx2);

        {
            let _guard = SessionGuard {
                cookie: "cookie-1".to_string(),
                sessions: Arc::clone(&sessions),
            };
        }
        let map = sessions.lock().await;
        assert!(!map.contains_key("cookie-1"));
        assert!(map.contains_key("cookie-2"));
    }

    #[tokio::test]
    async fn dispatcher_routes_response_to_correct_session() {
        let sessions: SessionMap = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let (resp_tx, resp_rx) = async_channel::bounded::<AuthResponse>(4);

        let (session_tx, mut session_rx) = tokio::sync::mpsc::channel::<AuthResponse>(1);
        sessions
            .lock()
            .await
            .insert("cookie-A".to_string(), session_tx);

        // Spawn the dispatcher logic inline
        let sessions_dispatch = Arc::clone(&sessions);
        tokio::spawn(async move {
            while let Ok(response) = resp_rx.recv().await {
                let session_tx = {
                    let map = sessions_dispatch.lock().await;
                    map.get(&response.cookie).cloned()
                };
                if let Some(session_tx) = session_tx {
                    let _ = session_tx.send(response).await;
                }
            }
        });

        // Send a response for cookie-A
        resp_tx
            .send(AuthResponse {
                cookie: "cookie-A".to_string(),
                password: None,
            })
            .await
            .unwrap();

        let received = session_rx.recv().await.unwrap();
        assert_eq!(received.cookie, "cookie-A");
        assert!(received.password.is_none());
    }
}
