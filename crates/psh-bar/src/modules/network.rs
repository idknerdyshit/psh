//! Network module — displays connection status via NetworkManager D-Bus.
//!
//! Connects to `org.freedesktop.NetworkManager` on the system bus and
//! monitors the active connection type, name, and signal strength.

use gtk4::glib;
use gtk4::prelude::*;

use super::{BarModule, ModuleContext};

/// Displays the current network connection status.
///
/// Shows connection type (ETH/WIFI/VPN) and name. Falls back to "---"
/// if NetworkManager is unavailable.
pub struct NetworkModule;

/// Parsed network connection state.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NetworkState {
    /// Connection type.
    pub conn_type: ConnectionType,
    /// Connection name (SSID, interface name, VPN name).
    pub name: String,
    /// Wifi signal strength percentage, if applicable.
    pub signal: Option<u8>,
}

/// Network connection type derived from NetworkManager.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConnectionType {
    /// Wired ethernet connection.
    Ethernet,
    /// Wireless (wifi) connection.
    Wifi,
    /// VPN or WireGuard tunnel.
    Vpn,
    /// Some other connection type.
    Other(String),
    /// No active connection.
    Disconnected,
}

/// NetworkManager D-Bus well-known name.
const NM_BUS: &str = "org.freedesktop.NetworkManager";
/// NM interface for the root manager object.
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
/// NM interface for active connection objects.
const NM_ACTIVE_CONN_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";
/// NM connection type for ethernet.
const NM_TYPE_ETHERNET: &str = "802-3-ethernet";
/// NM connection type for wifi.
const NM_TYPE_WIFI: &str = "802-11-wireless";

/// CSS class names used to indicate connection state.
const CONN_CLASSES: &[&str] = &["disconnected", "ethernet", "wifi", "vpn", "other"];

impl BarModule for NetworkModule {
    fn name(&self) -> &'static str {
        "network"
    }

    fn widget(&self, ctx: &ModuleContext) -> gtk4::Widget {
        let label = gtk4::Label::new(Some("---"));
        label.add_css_class("psh-bar-network");
        label.add_css_class("disconnected");

        let (tx, rx) = async_channel::bounded::<NetworkState>(4);

        ctx.rt.spawn(async move {
            if let Err(e) = run_nm_backend(tx).await {
                tracing::error!("network module backend error: {e}");
            }
        });

        let label_clone = label.clone();
        glib::spawn_future_local(async move {
            while let Ok(state) = rx.recv().await {
                let text = format_network_state(&state);
                label_clone.set_text(&text);

                for cls in CONN_CLASSES {
                    label_clone.remove_css_class(cls);
                }
                label_clone.add_css_class(css_class_for_state(&state));
            }
        });

        label.upcast()
    }
}

/// Run the NetworkManager D-Bus backend.
///
/// Connects to the system bus, queries initial state, and subscribes to
/// `PropertiesChanged` D-Bus signals for live updates. Falls back to 5s
/// polling if the signal subscription fails.
async fn run_nm_backend(
    tx: async_channel::Sender<NetworkState>,
) -> psh_core::Result<()> {
    use futures_util::StreamExt;
    use zbus::Connection;

    let conn = match Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("cannot connect to system bus for NetworkManager: {e}");
            return Ok(());
        }
    };

    // Send initial state
    let state = query_nm_state(&conn).await;
    if tx.send(state).await.is_err() {
        return Ok(());
    }

    // Try to subscribe to NM PropertiesChanged signals
    let proxy = zbus::fdo::PropertiesProxy::builder(&conn)
        .destination(NM_BUS)
        .ok()
        .and_then(|b| b.path("/org/freedesktop/NetworkManager").ok());

    let proxy = match proxy {
        Some(b) => b.build().await.ok(),
        None => None,
    };

    let signal_stream = match proxy {
        Some(p) => p.receive_properties_changed().await.ok(),
        None => None,
    };

    if let Some(mut signal_stream) = signal_stream {
        let fallback = std::time::Duration::from_secs(30);

        loop {
            tokio::select! {
                signal = signal_stream.next() => {
                    match signal {
                        Some(_) => {
                            let state = query_nm_state(&conn).await;
                            if tx.send(state).await.is_err() {
                                return Ok(());
                            }
                        }
                        None => {
                            tracing::warn!("NM signal stream ended, falling back to polling");
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep(fallback) => {
                    let state = query_nm_state(&conn).await;
                    if tx.send(state).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    } else {
        tracing::debug!("NM signal subscription failed, using 5s polling");
    }

    // Fallback: poll every 5 seconds
    loop {
        let state = query_nm_state(&conn).await;
        if tx.send(state).await.is_err() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

/// Query the current NetworkManager state via D-Bus properties.
async fn query_nm_state(conn: &zbus::Connection) -> NetworkState {
    let disconnected = NetworkState {
        conn_type: ConnectionType::Disconnected,
        name: String::new(),
        signal: None,
    };

    // Get NM state
    let nm_path = "/org/freedesktop/NetworkManager";
    let nm_state = get_dbus_property::<u32>(conn, nm_path, NM_IFACE, "State")
        .await
        .unwrap_or(0);

    if !nm_state_connected(nm_state) {
        return disconnected;
    }

    let primary_path = match get_dbus_property::<zbus::zvariant::OwnedObjectPath>(
        conn,
        nm_path,
        NM_IFACE,
        "PrimaryConnection",
    )
    .await
    {
        Some(p) if p.as_str() != "/" => p,
        _ => return disconnected,
    };

    let conn_type_str =
        get_dbus_property::<String>(conn, primary_path.as_str(), NM_ACTIVE_CONN_IFACE, "Type")
            .await
            .unwrap_or_default();
    let conn_name =
        get_dbus_property::<String>(conn, primary_path.as_str(), NM_ACTIVE_CONN_IFACE, "Id")
            .await
            .unwrap_or_default();

    let conn_type = parse_connection_type(&conn_type_str);

    // For wifi, try to get signal strength
    let signal = if conn_type == ConnectionType::Wifi {
        get_wifi_signal(conn, primary_path.as_str()).await
    } else {
        None
    };

    NetworkState {
        conn_type,
        name: conn_name,
        signal,
    }
}

/// Get a D-Bus property from a NetworkManager object.
async fn get_dbus_property<T>(
    conn: &zbus::Connection,
    path: &str,
    interface: &'static str,
    property: &str,
) -> Option<T>
where
    T: TryFrom<zbus::zvariant::OwnedValue>,
    T::Error: std::fmt::Debug,
{
    let proxy = zbus::fdo::PropertiesProxy::builder(conn)
        .destination(NM_BUS)
        .ok()?
        .path(path)
        .ok()?
        .build()
        .await
        .ok()?;

    let iface = zbus::names::InterfaceName::from_static_str_unchecked(interface);
    let value = proxy.get(iface, property).await.ok()?;

    T::try_from(value).ok()
}

/// NM interface for device objects.
const NM_DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
/// NM interface for wireless device objects.
const NM_WIRELESS_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wireless";
/// NM interface for access point objects.
const NM_AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";

/// Try to get wifi signal strength from the active access point.
///
/// Traverses the NM D-Bus device tree:
/// ActiveConnection → Devices → WirelessDevice → ActiveAccessPoint → Strength.
async fn get_wifi_signal(conn: &zbus::Connection, active_conn_path: &str) -> Option<u8> {
    // 1. Get devices from the active connection
    let devices = get_dbus_property::<Vec<zbus::zvariant::OwnedObjectPath>>(
        conn,
        active_conn_path,
        NM_ACTIVE_CONN_IFACE,
        "Devices",
    )
    .await?;

    // 2. Find the wireless device (DeviceType == 2)
    for dev_path in &devices {
        let dev_type =
            get_dbus_property::<u32>(conn, dev_path.as_str(), NM_DEVICE_IFACE, "DeviceType")
                .await
                .unwrap_or(0);

        if dev_type == 2 {
            // 3. Get active access point
            let ap_path = get_dbus_property::<zbus::zvariant::OwnedObjectPath>(
                conn,
                dev_path.as_str(),
                NM_WIRELESS_IFACE,
                "ActiveAccessPoint",
            )
            .await?;

            if ap_path.as_str() == "/" {
                return None;
            }

            // 4. Read signal strength
            return get_dbus_property::<u8>(conn, ap_path.as_str(), NM_AP_IFACE, "Strength")
                .await;
        }
    }

    None
}

/// Format a network state for display in the bar.
pub(crate) fn format_network_state(state: &NetworkState) -> String {
    match &state.conn_type {
        ConnectionType::Disconnected => "---".into(),
        ConnectionType::Ethernet => {
            if state.name.is_empty() {
                "ETH".into()
            } else {
                format!("ETH {}", state.name)
            }
        }
        ConnectionType::Wifi => {
            if let Some(signal) = state.signal {
                format!("WIFI {} {signal}%", state.name)
            } else {
                format!("WIFI {}", state.name)
            }
        }
        ConnectionType::Vpn => format!("VPN {}", state.name),
        ConnectionType::Other(t) => format!("{t} {}", state.name),
    }
}

/// CSS class name for the current connection type.
pub(crate) fn css_class_for_state(state: &NetworkState) -> &'static str {
    match &state.conn_type {
        ConnectionType::Disconnected => "disconnected",
        ConnectionType::Ethernet => "ethernet",
        ConnectionType::Wifi => "wifi",
        ConnectionType::Vpn => "vpn",
        ConnectionType::Other(_) => "other",
    }
}

/// Parse a NetworkManager connection type string.
pub(crate) fn parse_connection_type(nm_type: &str) -> ConnectionType {
    match nm_type {
        NM_TYPE_ETHERNET => ConnectionType::Ethernet,
        NM_TYPE_WIFI => ConnectionType::Wifi,
        "vpn" | "wireguard" => ConnectionType::Vpn,
        "" => ConnectionType::Disconnected,
        other => ConnectionType::Other(other.to_string()),
    }
}

/// Map a NetworkManager `NMState` u32 value to whether the network is connected.
pub(crate) fn nm_state_connected(state: u32) -> bool {
    // NM_STATE_CONNECTED_GLOBAL = 70
    state >= 70
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_disconnected() {
        let state = NetworkState {
            conn_type: ConnectionType::Disconnected,
            name: String::new(),
            signal: None,
        };
        assert_eq!(format_network_state(&state), "---");
    }

    #[test]
    fn format_ethernet_with_name() {
        let state = NetworkState {
            conn_type: ConnectionType::Ethernet,
            name: "enp0s3".into(),
            signal: None,
        };
        assert_eq!(format_network_state(&state), "ETH enp0s3");
    }

    #[test]
    fn format_ethernet_no_name() {
        let state = NetworkState {
            conn_type: ConnectionType::Ethernet,
            name: String::new(),
            signal: None,
        };
        assert_eq!(format_network_state(&state), "ETH");
    }

    #[test]
    fn format_wifi_with_signal() {
        let state = NetworkState {
            conn_type: ConnectionType::Wifi,
            name: "MyNetwork".into(),
            signal: Some(80),
        };
        assert_eq!(format_network_state(&state), "WIFI MyNetwork 80%");
    }

    #[test]
    fn format_wifi_no_signal() {
        let state = NetworkState {
            conn_type: ConnectionType::Wifi,
            name: "MyNetwork".into(),
            signal: None,
        };
        assert_eq!(format_network_state(&state), "WIFI MyNetwork");
    }

    #[test]
    fn format_vpn() {
        let state = NetworkState {
            conn_type: ConnectionType::Vpn,
            name: "work-vpn".into(),
            signal: None,
        };
        assert_eq!(format_network_state(&state), "VPN work-vpn");
    }

    #[test]
    fn parse_nm_connection_types() {
        assert_eq!(
            parse_connection_type("802-3-ethernet"),
            ConnectionType::Ethernet
        );
        assert_eq!(
            parse_connection_type("802-11-wireless"),
            ConnectionType::Wifi
        );
        assert_eq!(parse_connection_type("vpn"), ConnectionType::Vpn);
        assert_eq!(parse_connection_type("wireguard"), ConnectionType::Vpn);
        assert_eq!(
            parse_connection_type(""),
            ConnectionType::Disconnected
        );
        assert!(matches!(
            parse_connection_type("bridge"),
            ConnectionType::Other(_)
        ));
    }

    #[test]
    fn nm_state_values() {
        assert!(!nm_state_connected(0)); // NM_STATE_UNKNOWN
        assert!(!nm_state_connected(10)); // NM_STATE_ASLEEP
        assert!(!nm_state_connected(20)); // NM_STATE_DISCONNECTED
        assert!(!nm_state_connected(50)); // NM_STATE_CONNECTED_SITE
        assert!(nm_state_connected(70)); // NM_STATE_CONNECTED_GLOBAL
        assert!(nm_state_connected(80)); // higher values
    }

    #[test]
    fn css_classes() {
        let disconnected = NetworkState {
            conn_type: ConnectionType::Disconnected,
            name: String::new(),
            signal: None,
        };
        assert_eq!(css_class_for_state(&disconnected), "disconnected");

        let wifi = NetworkState {
            conn_type: ConnectionType::Wifi,
            name: "test".into(),
            signal: Some(50),
        };
        assert_eq!(css_class_for_state(&wifi), "wifi");
    }
}
