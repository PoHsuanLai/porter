//! accountd on the session bus, as a transport for `porter-client`: `Porter::over(Bus::over(c))`.
//! The caller's identity is what the bus says about the connection. This is the one crate of the
//! client side that names zbus, because an app that already runs zbus hands over its own
//! connection; `porter-client`'s own API names none.
//!
//! What accountd serves over the bus is the sheets that add an account and sign one in again, the
//! shell's view of the accounts, the identities, the Spaces, the person's computers and the AI keys
//! (`Keys`, `Launcher`, `Broker`). Grants, tokens and relays are not on the bus: they are the library's, for an app that hosts the service itself
//! (`porter-client-inproc`), so [`Bus`] does not implement `GrantsLink`.

mod accounts;
mod ai_accounts;
mod ai_keys;
mod credential;
mod failure;
mod identities;
mod launcher;
mod spaces;
mod tailnet;

pub use credential::{ChildKey, ChildKeyError, Delivery, Inherit, ProcessCredential, Revoked};
pub use launcher::{AskKind, KeyCandidate, Launcher, LauncherError, LauncherRequest};
use porter_client::{LinkError, Porter};

use porter_dbus::Destination;
use zbus::Connection;
use zbus::names::UniqueName;

/// accountd on the session bus. Calls go to its well-known name unless the caller names the owner
/// it checked ([`Bus::accounts_at`]).
#[derive(Debug, Clone)]
pub struct Bus {
    connection: Connection,
    accounts: Destination,
}

impl Bus {
    /// Over an open session-bus connection the app owns.
    pub fn over(connection: Connection) -> Self {
        Self {
            connection,
            accounts: Destination::WellKnown,
        }
    }

    /// The same, with every call to accountd sent to `owner`, the unique name the caller checked
    /// owns `org.quire.Accounts1`: a program that takes the name afterwards gets nothing.
    #[must_use]
    pub fn accounts_at(self, owner: UniqueName<'static>) -> Self {
        Self {
            accounts: Destination::Owner(owner),
            ..self
        }
    }

    /// Over a new connection to the session bus (`DBUS_SESSION_BUS_ADDRESS` or the runtime
    /// directory's socket, as the bus library resolves it).
    ///
    /// # Errors
    /// There is no session bus: [`LinkError::Unreachable`] (or what the bus library says).
    pub async fn session() -> Result<Self, LinkError> {
        Connection::session()
            .await
            .map(Self::over)
            .map_err(|error| failure::link_error(&error))
    }
}

/// A handle over a new connection to the session bus. The daemon is found, and started by bus
/// activation, at the first call.
///
/// # Errors
/// There is no session bus.
pub async fn connect() -> Result<Porter<Bus>, LinkError> {
    Bus::session().await.map(Porter::over)
}
