//! The D-Bus carrier: porter-dbus proxies on the session bus; the caller's identity is what
//! the bus says about the connection. The two sheet calls accountd still serves are built (adding
//! an identity and signing it in again). No credential crosses the bus any more, so nothing else
//! of accounts is carried: finding, choosing, tokens and relays are the library's, for an app that
//! hosts its own `AccountService` ([`super::InProcess`]), and every other call answers
//! `Unsupported`. Inference is `inferd-client`'s.

use super::Transport;
use crate::error::TransportError;
use porter_dbus::Destination;
use porter_dbus::{BusConnection, BusError, BusFailure, classify};
use porter_wire::{AccountsReply, AccountsRequest};
use zbus::names::UniqueName;

/// accountd on the session bus. Calls go to its well-known name unless the caller names the owner
/// it checked ([`Destination`]).
#[derive(Debug)]
pub struct DbusTransport {
    connection: BusConnection,
    accounts: Destination,
}

impl DbusTransport {
    /// Over an open session-bus connection the app owns.
    pub fn over(connection: BusConnection) -> Self {
        Self {
            connection,
            accounts: Destination::WellKnown,
        }
    }

    /// The same, with accountd's calls (the sheets, Spaces, identities, the tailnet and the
    /// removals watch) sent to `owner`, the unique name the caller checked owns
    /// `org.quire.Accounts1`.
    pub fn accounts_at(self, owner: UniqueName<'static>) -> Self {
        Self {
            accounts: Destination::Owner(owner),
            ..self
        }
    }

    /// Over a new connection to the session bus (`DBUS_SESSION_BUS_ADDRESS` or the runtime
    /// directory's socket, as the bus library resolves it).
    pub async fn session() -> Result<Self, TransportError> {
        BusConnection::session()
            .await
            .map(Self::over)
            .map_err(|e| bus_error(&e))
    }

    /// The connection, for the watches.
    pub(crate) fn connection(&self) -> &BusConnection {
        &self.connection
    }

    /// Where accountd's calls go.
    pub(crate) fn accounts(&self) -> &Destination {
        &self.accounts
    }

    /// accountd's registry of desktop-wide Spaces (`org.quire.Spaces1`), over this connection.
    pub async fn spaces(&self) -> Result<crate::Spaces, crate::SpacesError> {
        crate::Spaces::connect_at(&self.connection, &self.accounts).await
    }

    /// The identities signed in on this machine (`org.quire.Identities1`), over this connection:
    /// "Continue as" for an app that keeps its own sign-in.
    pub async fn identities(&self) -> Result<crate::Identities, crate::IdentitiesError> {
        crate::Identities::connect_at(&self.connection, &self.accounts).await
    }

    /// The person's computers on their Tailscale network (`org.quire.Tailnet1`), over this
    /// connection: for the shell, Settings and the terminal.
    pub async fn tailnet(&self) -> Result<crate::Tailnet, crate::CallFailure> {
        crate::Tailnet::connect_at(&self.connection, &self.accounts).await
    }
}

/// A bus error as the transport's: no daemon on the name or no bus is `Unreachable`; the bus's
/// `AccessDenied` (a caller the daemon does not know, or one without the grant) is `Denied` with
/// the daemon's text; anything else is the other side not speaking porter's protocol.
pub(crate) fn bus_error(error: &BusError) -> TransportError {
    match classify(error) {
        BusFailure::NoDaemon => TransportError::Unreachable,
        BusFailure::Denied(why) => TransportError::Denied(why),
        BusFailure::Other(why) => TransportError::Malformed(format!("bus: {why}")),
        // a failure a later porter-dbus adds
        _ => TransportError::Malformed("a bus failure this client does not know".to_owned()),
    }
}

impl Transport for DbusTransport {
    async fn call(&self, request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        super::dbus_accounts::call(&self.connection, &self.accounts, request).await
    }
}
