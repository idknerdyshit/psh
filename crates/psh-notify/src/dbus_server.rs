use std::sync::atomic::{AtomicU32, Ordering};

use async_channel::Sender;
use zbus::{connection::Builder as ConnectionBuilder, interface};

use psh_core::Result;

static NOTIFICATION_ID: AtomicU32 = AtomicU32::new(1);

#[derive(Debug, Clone)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub summary: String,
    pub body: String,
    pub expire_timeout: i32,
}

pub struct NotificationServer {
    tx: Sender<Notification>,
}

#[interface(name = "org.freedesktop.Notifications")]
impl NotificationServer {
    async fn get_capabilities(&self) -> Vec<String> {
        vec![
            "body".into(),
            "body-markup".into(),
            "actions".into(),
        ]
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        _hints: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id > 0 {
            replaces_id
        } else {
            NOTIFICATION_ID.fetch_add(1, Ordering::Relaxed)
        };

        let notif = Notification {
            id,
            app_name,
            summary,
            body,
            expire_timeout,
        };

        tracing::debug!("notification #{id}: {}", notif.summary);
        let _ = self.tx.send(notif).await;
        id
    }

    async fn close_notification(&self, _id: u32) {}

    async fn get_server_information(&self) -> (String, String, String, String) {
        (
            "psh-notify".into(),
            "psh".into(),
            env!("CARGO_PKG_VERSION").into(),
            "1.2".into(),
        )
    }
}

pub async fn run(tx: Sender<Notification>) -> Result<()> {
    let server = NotificationServer { tx };

    let _conn = ConnectionBuilder::session()?
        .name("org.freedesktop.Notifications")?
        .serve_at("/org/freedesktop/Notifications", server)?
        .build()
        .await?;

    tracing::info!("notification dbus server running");

    // Keep the connection alive
    std::future::pending::<()>().await;
    Ok(())
}
