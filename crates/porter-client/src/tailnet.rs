//! The person's own computers on their Tailscale network (`org.quire.Tailnet1` on accountd), for
//! the shell, Settings and the terminal. Nothing here reads Tailscale: accountd does, once, for
//! the Tailscale account the person added.
//!
//! ```ignore
//! let tailnet = transport.tailnet().await?;
//! let mut changes = tailnet.watch().await?;          // subscribe before reading the list
//! for machine in tailnet.machines().await? {          // Machine { node, name, dns, owner, .. }
//!     /* connect to machine.dns; key pins and consent by machine.node */
//! }
//! while changes.next().await.is_some() { /* read the list again */ }
//! ```

use porter_core::{CoreError, Machine};
use porter_dbus::{
    BusConnection, BusError, BusFailure, BusStream, TailnetChanged, TailnetChangedStream,
    TailnetProxy, classify, machine_from_dbus,
};
use std::pin::Pin;
use std::task::{Context, Poll};

/// Why a Tailnet call failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TailnetError {
    /// accountd does not know this caller, or it is not the shell, Settings or the terminal.
    #[error("refused by accountd: {0}")]
    Denied(String),
    /// accountd is not there.
    #[error("no account service reachable")]
    Unreachable,
    /// accountd sent something that is not porter's protocol.
    #[error("malformed: {0}")]
    Malformed(String),
}

impl From<BusError> for TailnetError {
    fn from(error: BusError) -> Self {
        match classify(&error) {
            BusFailure::NoDaemon => TailnetError::Unreachable,
            BusFailure::Denied(why) => TailnetError::Denied(why),
            BusFailure::Other(why) => TailnetError::Malformed(why),
        }
    }
}

impl From<CoreError> for TailnetError {
    fn from(error: CoreError) -> Self {
        TailnetError::Malformed(error.to_string())
    }
}

/// accountd's list of the person's computers, as its callers see it.
#[derive(Debug, Clone)]
pub struct Tailnet {
    proxy: TailnetProxy<'static>,
}

impl Tailnet {
    /// The list over `connection`.
    pub async fn connect(connection: &BusConnection) -> Result<Self, TailnetError> {
        Ok(Self {
            proxy: TailnetProxy::new(connection).await?,
        })
    }

    /// The other computers on the person's network, none for this one, in accountd's order. The
    /// list is empty with no Tailscale account, or one that is not working: the account's own
    /// state says why.
    pub async fn machines(&self) -> Result<Vec<Machine>, TailnetError> {
        self.proxy
            .machines()
            .await?
            .iter()
            .map(|row| Ok(machine_from_dbus(row)?))
            .collect()
    }

    /// The changes to the list, from now on. It subscribes, then reads the list once so
    /// accountd counts this connection among those it tells.
    pub async fn watch(&self) -> Result<MachineChanges, TailnetError> {
        let stream = self.proxy.receive_changed().await?;
        self.proxy.machines().await?;
        Ok(MachineChanges(stream))
    }
}

/// The stream of changes: one item each time the list changed, with nothing in it; read the list
/// again.
#[derive(Debug)]
pub struct MachineChanges(TailnetChangedStream);

impl BusStream for MachineChanges {
    type Item = ();

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
            .poll_next(cx)
            .map(|next| next.map(|_signal: TailnetChanged| ()))
    }
}
