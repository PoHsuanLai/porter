//! The D-Bus carrier: porter-dbus proxies on the session bus; the caller's identity is what
//! the bus says about the connection. `Inference1.Open` is built (a socket fd, framed with
//! `porter_core::wire`), and so are accountd's immediate calls; the sheet calls wait for accountd.

#[cfg(feature = "infer")]
use super::NewComputer;
use super::Transport;
#[cfg(feature = "infer")]
use super::dbus_session::DbusSession;
use crate::authenticated::{AuthenticatedStream, Relayed};
use crate::error::TransportError;
use porter_core::lending::{ComputerCandidate, GuestAnswer, GuestRow};
use porter_core::{AccountsReply, AccountsRequest, EndpointUrl, GrantId, NodeId};
#[cfg(feature = "infer")]
use porter_core::{DataClass, Need, Tier};
use porter_dbus::{BusConnection, BusError, BusFailure, TokensProxy, classify, refusal_of};
#[cfg(feature = "infer")]
use porter_infer::{ComputerName, OpenOptions, PlaceId, PlaceRow, Readiness};
use serde::Serialize;

/// accountd and inferd on the session bus.
#[derive(Debug)]
pub struct DbusTransport {
    connection: BusConnection,
}

impl DbusTransport {
    /// Over an open session-bus connection the app owns.
    pub fn over(connection: BusConnection) -> Self {
        Self { connection }
    }

    /// Over a new connection to the session bus (`DBUS_SESSION_BUS_ADDRESS` or the runtime
    /// directory's socket, as the bus library resolves it).
    pub async fn session() -> Result<Self, TransportError> {
        BusConnection::session()
            .await
            .map(Self::over)
            .map_err(|e| bus_error(&e))
    }

    /// The connection, for the inference calls and the removals watch.
    pub(crate) fn connection(&self) -> &BusConnection {
        &self.connection
    }

    /// accountd's registry of desktop-wide Spaces (`org.quire.Spaces1`), over this connection.
    pub async fn spaces(&self) -> Result<crate::Spaces, crate::SpacesError> {
        crate::Spaces::connect(&self.connection).await
    }

    /// The person's computers on their Tailscale network (`org.quire.Tailnet1`), over this
    /// connection: for the shell, Settings and the terminal.
    pub async fn tailnet(&self) -> Result<crate::Tailnet, crate::TailnetError> {
        crate::Tailnet::connect(&self.connection).await
    }
}

/// A closed set's serde form is its slug on the bus too.
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
        super::dbus_accounts::call(&self.connection, request).await
    }

    async fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<Relayed, TransportError> {
        let proxy = TokensProxy::new(&self.connection)
            .await
            .map_err(|e| bus_error(&e))?;
        match proxy
            .open_authenticated(grant.as_str(), endpoint.as_str())
            .await
        {
            Ok(fd) => Ok(Relayed::Stream(AuthenticatedStream::Fd(fd.into()))),
            Err(error) => match refusal_of(&error) {
                Some(refusal) => Ok(Relayed::Refused(refusal)),
                None => Err(bus_error(&error)),
            },
        }
    }

    async fn open_linked(
        &self,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<Relayed, TransportError> {
        let proxy = TokensProxy::new(&self.connection)
            .await
            .map_err(|e| bus_error(&e))?;
        match proxy.open_linked(grant.as_str(), origin.as_str()).await {
            Ok(fd) => Ok(Relayed::Stream(AuthenticatedStream::Fd(fd.into()))),
            Err(error) => match refusal_of(&error) {
                Some(refusal) => Ok(Relayed::Refused(refusal)),
                None => Err(bus_error(&error)),
            },
        }
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
        self.list_guests().await
    }

    async fn answer_guest(&self, node: &NodeId, answer: GuestAnswer) -> Result<(), TransportError> {
        self.send_guest_answer(node, answer).await
    }

    async fn forget_guest(&self, node: &NodeId) -> Result<(), TransportError> {
        self.send_forget_guest(node).await
    }

    async fn candidates(&self) -> Result<Vec<ComputerCandidate>, TransportError> {
        self.list_candidates().await
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
        self.list_places().await
    }
}
