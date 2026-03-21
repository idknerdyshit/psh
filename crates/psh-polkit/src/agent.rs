use async_channel::{Receiver, Sender};
use zbus::{connection::Builder as ConnectionBuilder, interface};

use psh_core::Result;

#[derive(Debug, Clone)]
pub struct AuthRequest {
    pub cookie: String,
    pub message: String,
    pub icon_name: String,
    pub identities: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AuthResponse {
    pub cookie: String,
    pub password: Option<String>,
}

pub struct PolkitAgent {
    tx: Sender<AuthRequest>,
    rx: Receiver<AuthResponse>,
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl PolkitAgent {
    async fn begin_authentication(
        &self,
        action_id: &str,
        message: &str,
        icon_name: &str,
        _details: std::collections::HashMap<String, String>,
        cookie: &str,
        identities: Vec<(String, std::collections::HashMap<String, zbus::zvariant::OwnedValue>)>,
    ) {
        tracing::info!("polkit auth request: action={action_id} message={message}");

        let identity_names: Vec<String> = identities
            .iter()
            .map(|(kind, _)| kind.clone())
            .collect();

        let req = AuthRequest {
            cookie: cookie.to_string(),
            message: message.to_string(),
            icon_name: icon_name.to_string(),
            identities: identity_names,
        };

        let _ = self.tx.send(req).await;

        // Wait for the UI response
        if let Ok(resp) = self.rx.recv().await {
            if resp.password.is_some() {
                tracing::info!("auth response received for cookie {}", resp.cookie);
            } else {
                tracing::info!("auth cancelled for cookie {}", resp.cookie);
            }
        }
    }

    async fn cancel_authentication(&self, cookie: &str) {
        tracing::info!("auth cancelled by system: cookie={cookie}");
    }
}

pub async fn run(tx: Sender<AuthRequest>, rx: Receiver<AuthResponse>) -> Result<()> {
    let agent = PolkitAgent { tx, rx };

    let _conn = ConnectionBuilder::system()?
        .serve_at("/org/freedesktop/PolicyKit1/AuthenticationAgent", agent)?
        .build()
        .await?;

    tracing::info!("polkit agent registered");
    std::future::pending::<()>().await;
    Ok(())
}
