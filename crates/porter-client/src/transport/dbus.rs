//! The D-Bus carrier: porter-dbus proxies on the session bus; the caller's identity is what
//! the bus says about the connection. `Inference1.Open` is built (a socket fd, framed with
//! `porter_core::wire`), and so are accountd's immediate calls; the sheet calls wait for accountd.

use super::Transport;
#[cfg(feature = "infer")]
use super::dbus_session::DbusSession;
use crate::authenticated::{AuthenticatedStream, Relayed};
use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest, EndpointUrl, GrantId};
#[cfg(feature = "infer")]
use porter_core::{DataClass, Need, Tier};
use porter_dbus::{BusConnection, BusError, BusFailure, TokensProxy, classify, refusal_of};
#[cfg(feature = "infer")]
use porter_infer::{OpenOptions, PlaceRow, Readiness};
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

    /// The connection, for the inference calls.
    #[cfg(feature = "infer")]
    pub(super) fn connection(&self) -> &BusConnection {
        &self.connection
    }

    /// accountd's registry of desktop-wide Spaces (`org.quire.Spaces1`), over this connection.
    pub async fn spaces(&self) -> Result<crate::Spaces, crate::SpacesError> {
        crate::Spaces::connect(&self.connection).await
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
pub(super) fn bus_error(error: &BusError) -> TransportError {
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
