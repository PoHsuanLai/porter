//! The D-Bus carrier to inferd: porter-dbus proxies on the session bus; the caller's identity is
//! what the bus says about the connection. `Inference1.Open` is built (a socket fd, framed with
//! `porter_wire`), and so are the calls about computers, guests and the models inferd runs.

use super::NewComputer;
use super::Transport;
use super::dbus_session::DbusSession;
use crate::error::TransportError;
use porter_capability::Tier;
use porter_core::DataClass;
use porter_dbus::Destination;
use porter_dbus::{BusConnection, BusError, BusFailure, classify};
use porter_infer::{ComputerName, OpenOptions, PlaceId, PlaceRow, Readiness};
use porter_match::Need;
use porter_tailnet::{ComputerCandidate, GuestAnswer, GuestRow};
use porter_tailscale::NodeId;
use serde::Serialize;
use zbus::names::UniqueName;

/// inferd on the session bus. Calls go to its well-known name unless the caller names the owner
/// it checked ([`DbusTransport::inference_at`]).
#[derive(Debug)]
pub struct DbusTransport {
    connection: BusConnection,
    inference: Destination,
}

impl DbusTransport {
    /// Over an open session-bus connection the app owns.
    pub fn over(connection: BusConnection) -> Self {
        Self {
            connection,
            inference: Destination::WellKnown,
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

    /// Where inferd's calls go.
    pub(crate) fn inference(&self) -> &Destination {
        &self.inference
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
    if let Some((reason, kind)) = porter_dbus::inference::place_refusal_of(error)
        && let Some(reason) = porter_infer::NoPlaceReason::from_name(&reason)
    {
        return TransportError::NoAllowedPlace {
            reason,
            would_need: kind.and_then(|slug| porter_infer::PlaceKind::from_slug(&slug)),
        };
    }
    // inferd's plain refusal of a call about computers or guests.
    if let Some((name, words)) = porter_dbus::inference::computer_refusal_of(error) {
        return TransportError::Computer {
            reason: crate::error::ComputerReason::from_name(&name),
            words,
        };
    }
    match classify(error) {
        BusFailure::NoDaemon => TransportError::Unreachable,
        BusFailure::Denied(why) => TransportError::Denied(why),
        BusFailure::Other(why) => TransportError::Malformed(format!("bus: {why}")),
        // a failure a later porter-dbus adds
        _ => TransportError::Malformed("a bus failure this client does not know".to_owned()),
    }
}

impl Transport for DbusTransport {
    type Session = DbusSession;

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

    async fn add_tailnet_computer(&self, node: &NodeId) -> Result<PlaceId, TransportError> {
        self.send_add_tailnet_computer(node).await
    }

    async fn add_computer(&self, computer: &NewComputer) -> Result<PlaceId, TransportError> {
        self.send_add_computer(computer).await
    }

    async fn remove_computer(&self, name: &ComputerName) -> Result<(), TransportError> {
        self.send_remove_computer(name).await
    }

    async fn set_helper_pool(
        &self,
        slot: porter_infer::Slot,
        helper: &porter_core::HelperId,
        models: &[porter_infer::ModelRef],
    ) -> Result<(), TransportError> {
        self.send_helper_pool(slot, helper, models).await
    }

    async fn prepare(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<Readiness, TransportError> {
        self.prepare_engine(need, class, tier, options).await
    }

    async fn places(&self) -> Result<Vec<PlaceRow>, TransportError> {
        Ok(self.list_places().await?.rows)
    }
}
