//! Which bus a daemon connects to: the person's session bus, or one at an address it is given.
//!
//! A daemon takes this in its configuration instead of reading `DBUS_SESSION_BUS_ADDRESS`
//! itself, so a program that runs the daemon inside it (a test on a private bus, a host that
//! embeds it) says where the bus is.

use zbus::Connection;
use zbus::connection::Builder;

/// The bus to connect to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BusTarget {
    /// The person's session bus, found the usual way.
    Session,
    /// The bus listening at this address (`unix:path=...`).
    Address(String),
}

impl BusTarget {
    /// A connection to the bus.
    ///
    /// # Errors
    /// The address is not one, or the bus cannot be reached.
    pub async fn connect(&self) -> zbus::Result<Connection> {
        match self {
            Self::Session => Builder::session()?.build().await,
            Self::Address(address) => Builder::address(address.as_str())?.build().await,
        }
    }
}
