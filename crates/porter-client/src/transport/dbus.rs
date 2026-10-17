//! The D-Bus carrier: porter-dbus proxies on the session bus; the caller's identity is what
//! the bus says about the connection. `Inference1.Open` is built (a socket fd, framed with
//! `porter_core::wire`), and so are the two sheet calls accountd still serves (adding an identity
//! and signing it in again). No credential crosses the bus any more, so nothing else of accounts
//! is carried: finding, choosing, tokens and relays are the library's, for an app that hosts its
//! own `AccountService` ([`super::InProcess`]), and every other call answers `Unsupported`.

#[cfg(feature = "infer")]
use super::NewComputer;
use super::Transport;
#[cfg(feature = "infer")]
use super::dbus_session::DbusSession;
use crate::Destination;
use crate::error::TransportError;
use porter_core::lending::{ComputerCandidate, GuestAnswer, GuestRow};
use porter_core::{AccountsReply, AccountsRequest, NodeId};
#[cfg(feature = "infer")]
use porter_core::{DataClass, Need, Tier};
use porter_dbus::{BusConnection, BusError, BusFailure, classify};
#[cfg(feature = "infer")]
use porter_infer::{ComputerName, OpenOptions, PlaceId, PlaceRow, Readiness};
#[cfg(feature = "infer")]
use serde::Serialize;
use zbus::names::UniqueName;

/// inferd on the session bus, and accountd for the sheets it still shows. Calls go to each
/// daemon's well-known name unless the caller names the owner it checked ([`Destination`]).
#[derive(Debug)]
pub struct DbusTransport {
    connection: BusConnection,
    accounts: Destination,
    inference: Destination,
}

impl DbusTransport {
    /// Over an open session-bus connection the app owns.
    pub fn over(connection: BusConnection) -> Self {
        Self {
            connection,
            accounts: Destination::WellKnown,
            inference: Destination::WellKnown,
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

    /// The same, with inferd's calls (sessions, places, computers, guests and the guests watch)
    /// sent to `owner`, the unique name the caller checked owns `org.quire.Inference1`.
    pub fn inference_at(self, owner: UniqueName<'static>) -> Self {
        Self {
            inference: Destination::Owner(owner),
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

    /// The connection, for the inference calls and the watches.
    pub(crate) fn connection(&self) -> &BusConnection {
        &self.connection
    }

    /// Where accountd's calls go.
    pub(crate) fn accounts(&self) -> &Destination {
        &self.accounts
    }

    /// Where inferd's calls go.
    pub(crate) fn inference(&self) -> &Destination {
        &self.inference
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
    pub async fn tailnet(&self) -> Result<crate::Tailnet, crate::TailnetError> {
        crate::Tailnet::connect_at(&self.connection, &self.accounts).await
    }
}

/// The rows of a list the daemon sent that could be read, and why each of the others could not.
/// One row that does not say what the interface promises is left out; it never fails the list.
/// Only a reply that is not a list at all is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rows<T> {
    /// The rows that were read, in the order the daemon sent them.
    pub rows: Vec<T>,
    /// One [`TransportError::Malformed`] for each row that was left out, naming the row.
    pub skipped: Vec<TransportError>,
}

impl<T> Rows<T> {
    /// Reads `rows` one by one with `decode`: a row that fails is put in `skipped`.
    pub(super) fn decode<R>(
        rows: impl IntoIterator<Item = R>,
        decode: impl Fn(R) -> Result<T, TransportError>,
    ) -> Self {
        let mut read = Vec::new();
        let mut skipped = Vec::new();
        for row in rows {
            match decode(row) {
                Ok(row) => read.push(row),
                Err(error) => skipped.push(error),
            }
        }
        Self {
            rows: read,
            skipped,
        }
    }
}

/// A closed set's serde form is its slug on the bus too. Only inference's calls send one now.
#[cfg(feature = "infer")]
pub(super) fn slug<T: Serialize + ?Sized>(value: &T) -> Result<String, TransportError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        _ => Err(TransportError::Malformed("not a slug".to_owned())),
    }
}

/// A bus error as the transport's: no daemon on the name or no bus is `Unreachable`; the bus's
/// `AccessDenied` (a caller the daemon does not know, or one without the grant) is `Denied` with
/// the daemon's text; anything else is the other side not speaking porter's protocol.
pub(crate) fn bus_error(error: &BusError) -> TransportError {
    // inferd's answer to a call that named its places and has none to run in.
    #[cfg(feature = "infer")]
    if let Some((reason, kind)) = porter_dbus::place_refusal_of(error)
        && let Some(reason) = porter_infer::NoPlaceReason::from_name(&reason)
    {
        return TransportError::NoAllowedPlace {
            reason,
            would_need: kind.and_then(|slug| porter_infer::PlaceKind::from_slug(&slug)),
        };
    }
    // inferd's plain refusal of a call about computers or guests.
    if let Some((name, words)) = porter_dbus::computer_refusal_of(error) {
        return TransportError::Computer {
            reason: crate::error::ComputerReason::from_name(&name),
            words,
        };
    }
    match classify(error) {
        BusFailure::NoDaemon => TransportError::Unreachable,
        BusFailure::Denied(why) => TransportError::Denied(why),
        BusFailure::Other(why) => TransportError::Malformed(format!("bus: {why}")),
    }
}

impl Transport for DbusTransport {
    #[cfg(feature = "infer")]
    type Session = DbusSession;

    async fn call(&self, request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        super::dbus_accounts::call(&self.connection, &self.accounts, request).await
    }

    #[cfg(feature = "infer")]
    async fn open_with(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<DbusSession, TransportError> {
        self.open_session(need, class, tier, options).await
    }

    async fn guests(&self) -> Result<Vec<GuestRow>, TransportError> {
        Ok(self.list_guests().await?.rows)
    }

    async fn answer_guest(&self, node: &NodeId, answer: GuestAnswer) -> Result<(), TransportError> {
        self.send_guest_answer(node, answer).await
    }

    async fn forget_guest(&self, node: &NodeId) -> Result<(), TransportError> {
        self.send_forget_guest(node).await
    }

    async fn candidates(&self) -> Result<Vec<ComputerCandidate>, TransportError> {
        Ok(self.list_candidates().await?.rows)
    }

    #[cfg(feature = "infer")]
    async fn add_tailnet_computer(&self, node: &NodeId) -> Result<PlaceId, TransportError> {
        self.send_add_tailnet_computer(node).await
    }

    #[cfg(feature = "infer")]
    async fn add_computer(&self, computer: &NewComputer) -> Result<PlaceId, TransportError> {
        self.send_add_computer(computer).await
    }

    #[cfg(feature = "infer")]
    async fn remove_computer(&self, name: &ComputerName) -> Result<(), TransportError> {
        self.send_remove_computer(name).await
    }

    #[cfg(feature = "infer")]
    async fn prepare(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<Readiness, TransportError> {
        self.prepare_engine(need, class, tier, options).await
    }

    #[cfg(feature = "infer")]
    async fn places(&self) -> Result<Vec<PlaceRow>, TransportError> {
        Ok(self.list_places().await?.rows)
    }
}
