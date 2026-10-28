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

use crate::call_failure::CallFailure;
use porter_dbus::accounts::{
    TailnetChanged, TailnetChangedStream, TailnetProxy, machine_from_dbus,
};
use porter_dbus::{BusConnection, BusStream};
use porter_tailscale::Machine;
use std::pin::Pin;
use std::task::{Context, Poll};

/// accountd's list of the person's computers, as its callers see it.
#[derive(Debug, Clone)]
pub struct Tailnet {
    proxy: TailnetProxy<'static>,
}

impl Tailnet {
    /// The list over `connection`.
    pub async fn connect(connection: &BusConnection) -> Result<Self, CallFailure> {
        Self::connect_at(connection, &porter_dbus::Destination::WellKnown).await
    }

    /// The same, sent to `to`: the owner of accountd's name the caller checked.
    pub async fn connect_at(
        connection: &BusConnection,
        to: &porter_dbus::Destination,
    ) -> Result<Self, CallFailure> {
        Ok(Self {
            proxy: to.proxy(connection).await?,
        })
    }

    /// The other computers on the person's network, none for this one, in accountd's order. The
    /// list is empty with no Tailscale account, or one that is not working: the account's own
    /// state says why.
    pub async fn machines(&self) -> Result<Vec<Machine>, CallFailure> {
        self.proxy
            .machines()
            .await?
            .iter()
            .map(|row| Ok(machine_from_dbus(row)?))
            .collect()
    }

    /// The changes to the list, from now on. It subscribes, then reads the list once so
    /// accountd counts this connection among those it tells.
    pub async fn watch(&self) -> Result<MachineChanges, CallFailure> {
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
