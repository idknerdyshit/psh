use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use async_channel::{Receiver, Sender};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;
use zbus::{connection::Builder as ConnectionBuilder, interface};

use psh_core::Result;

static NOTIFICATION_ID: AtomicU32 = AtomicU32::new(1);

/// Allocate the next notification ID, skipping 0 (which means "assign new" per the fd.o spec).
fn next_id() -> u32 {
    loop {
        let id = NOTIFICATION_ID.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            return id;
        }
    }
}

// ---------------------------------------------------------------------------
// Types shared between D-Bus thread and GTK thread
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Low = 0,
    Normal = 1,
    Critical = 2,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ImageData {
    pub width: i32,
    pub height: i32,
    pub rowstride: i32,
    pub has_alpha: bool,
    pub bits_per_sample: i32,
    pub channels: i32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    pub urgency: Urgency,
    pub image_data: Option<ImageData>,
    pub expire_timeout: i32,
    pub replaces_id: u32,
}

/// Messages sent from the D-Bus thread to the GTK thread.
#[derive(Debug)]
pub enum DbusToGtk {
    Notify(Notification),
    Close(u32),
}

/// Messages sent from the GTK thread back to the D-Bus thread for signal emission.
#[derive(Debug)]
pub enum GtkToDbus {
    Closed { id: u32, reason: u32 },
    ActionInvoked { id: u32, action_key: String },
}

// ---------------------------------------------------------------------------
// D-Bus interface
// ---------------------------------------------------------------------------

pub struct NotificationServer {
    tx: Sender<DbusToGtk>,
    /// Track IDs issued by this server to prevent replaces_id injection.
    issued_ids: Mutex<HashSet<u32>>,
}

#[interface(name = "org.freedesktop.Notifications")]
impl NotificationServer {
    async fn get_capabilities(&self) -> Vec<String> {
        vec![
            "body".into(),
            "body-markup".into(),
            "actions".into(),
            "icon-static".into(),
        ]
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id > 0
            && self
                .issued_ids
                .lock()
                .unwrap()
                .contains(&replaces_id)
        {
            replaces_id
        } else {
            next_id()
        };
        self.issued_ids.lock().unwrap().insert(id);

        let parsed_actions = parse_actions(&actions);
        let urgency = parse_urgency(&hints);
        let image_data = parse_image_data(&hints);

        let notif = Notification {
            id,
            app_name,
            app_icon,
            summary,
            body,
            actions: parsed_actions,
            urgency,
            image_data,
            expire_timeout,
            replaces_id,
        };

        tracing::debug!(
            "notification #{id}: {} (urgency={:?})",
            notif.summary,
            urgency
        );
        let _ = self.tx.send(DbusToGtk::Notify(notif)).await;
        id
    }

    async fn close_notification(&self, id: u32) {
        tracing::debug!("CloseNotification #{id}");
        self.issued_ids.lock().unwrap().remove(&id);
        let _ = self.tx.send(DbusToGtk::Close(id)).await;
    }

    async fn get_server_information(&self) -> (String, String, String, String) {
        (
            "psh-notify".into(),
            "psh".into(),
            env!("CARGO_PKG_VERSION").into(),
            "1.2".into(),
        )
    }

    /// D-Bus signal: emitted when a notification is closed.
    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    /// D-Bus signal: emitted when a notification action is invoked.
    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

// ---------------------------------------------------------------------------
// Hint parsing helpers
// ---------------------------------------------------------------------------

/// Parse D-Bus actions array (alternating key, label pairs) into Vec<(key, label)>.
fn parse_actions(actions: &[String]) -> Vec<(String, String)> {
    actions
        .chunks_exact(2)
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect()
}

/// Extract urgency from hints. Defaults to Normal.
///
/// Handles both a raw byte and a variant-wrapped byte (some older libnotify
/// versions wrap the urgency in an extra D-Bus variant layer).
fn parse_urgency(hints: &HashMap<String, OwnedValue>) -> Urgency {
    if let Some(val) = hints.get("urgency")
        && let Some(u) = extract_urgency_byte(val)
    {
        return match u {
            0 => Urgency::Low,
            2 => Urgency::Critical,
            _ => Urgency::Normal,
        };
    }
    Urgency::Normal
}

/// Try to extract a u8 from a zvariant value, unwrapping one layer of Variant if needed.
fn extract_urgency_byte(val: &OwnedValue) -> Option<u8> {
    if let Ok(u) = <u8>::try_from(val) {
        return Some(u);
    }
    // Some clients double-wrap: Variant(Variant(byte))
    let inner: zbus::zvariant::Value = val.try_into().ok()?;
    <u8>::try_from(&inner).ok()
}

/// Extract image-data hint. Format: (iiibiiay).
/// Checks all three hint keys from the fd.o spec: `image-data`, `image_data`, `icon_data`.
fn parse_image_data(hints: &HashMap<String, OwnedValue>) -> Option<ImageData> {
    let val = hints
        .get("image-data")
        .or_else(|| hints.get("image_data"))
        .or_else(|| hints.get("icon_data"))?;

    // Try to destructure the (iiibiiay) structure
    let structure = val.downcast_ref::<zbus::zvariant::Structure>().ok()?;
    let fields = structure.fields();
    if fields.len() != 7 {
        return None;
    }

    let width = i32::try_from(&fields[0]).ok()?;
    let height = i32::try_from(&fields[1]).ok()?;
    let rowstride = i32::try_from(&fields[2]).ok()?;
    let has_alpha = bool::try_from(&fields[3]).ok()?;
    let bits_per_sample = i32::try_from(&fields[4]).ok()?;
    let channels = i32::try_from(&fields[5]).ok()?;

    // Validate dimensions are positive and data length is sufficient
    if width <= 0 || height <= 0 || rowstride <= 0 || bits_per_sample <= 0 || channels <= 0 {
        tracing::warn!("image-data: invalid dimensions w={width} h={height} rs={rowstride}");
        return None;
    }
    let expected_len = (rowstride as u64).checked_mul(height as u64)?;
    if expected_len > 100_000_000 {
        tracing::warn!("image-data: unreasonable size ({expected_len} bytes), rejecting");
        return None;
    }

    // Extract byte array from the Array value
    let data = match &fields[6] {
        zbus::zvariant::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| u8::try_from(v).ok())
            .collect::<Vec<u8>>(),
        _ => return None,
    };

    if (data.len() as u64) < expected_len {
        tracing::warn!(
            "image-data: data too short ({} < {expected_len})",
            data.len()
        );
        return None;
    }

    Some(ImageData {
        width,
        height,
        rowstride,
        has_alpha,
        bits_per_sample,
        channels,
        data,
    })
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

/// Run the D-Bus notification server. Sends notifications to `forward_tx`,
/// receives signal requests from `signal_rx`.
pub async fn run(forward_tx: Sender<DbusToGtk>, signal_rx: Receiver<GtkToDbus>) -> Result<()> {
    let server = NotificationServer {
        tx: forward_tx,
        issued_ids: Mutex::new(HashSet::new()),
    };

    let conn = ConnectionBuilder::session()?
        .name("org.freedesktop.Notifications")?
        .serve_at("/org/freedesktop/Notifications", server)?
        .build()
        .await?;

    tracing::info!("notification dbus server running");

    // Spawn a task to emit D-Bus signals from GTK thread requests
    let conn_clone = conn.clone();
    tokio::spawn(async move {
        while let Ok(event) = signal_rx.recv().await {
            let iface_ref = match conn_clone
                .object_server()
                .interface::<_, NotificationServer>("/org/freedesktop/Notifications")
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("failed to get interface for signal emission: {e}");
                    continue;
                }
            };
            let emitter = iface_ref.signal_emitter();
            match event {
                GtkToDbus::Closed { id, reason } => {
                    if let Err(e) =
                        NotificationServer::notification_closed(emitter, id, reason).await
                    {
                        tracing::warn!("failed to emit NotificationClosed: {e}");
                    }
                }
                GtkToDbus::ActionInvoked { id, action_key } => {
                    if let Err(e) =
                        NotificationServer::action_invoked(emitter, id, &action_key).await
                    {
                        tracing::warn!("failed to emit ActionInvoked: {e}");
                    }
                }
            }
        }
    });

    // Keep the connection alive
    std::future::pending::<()>().await;
    Ok(())
}
