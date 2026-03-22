use tracing::info;
use zbus::Connection;
use zbus::connection::Builder;

use crate::Result;

/// Connect to the session bus.
pub async fn session_bus() -> Result<Connection> {
    let conn = Connection::session().await?;
    Ok(conn)
}

/// Connect to the session bus and request a well-known name.
pub async fn session_bus_with_name(name: &str) -> Result<Connection> {
    let conn = Builder::session()?.name(name)?.build().await?;
    info!("acquired dbus name: {name}");
    Ok(conn)
}

/// Connect to the system bus.
pub async fn system_bus() -> Result<Connection> {
    let conn = Connection::system().await?;
    Ok(conn)
}
