//! Where a bus call goes: a daemon's well-known name, or the unique name of the connection a
//! caller has already checked owns it. A well-known name can change hands between that check
//! and the call (another program takes it the moment the daemon lets go), so a caller that
//! checked who owns `org.quire.Inference1` sends to that owner's unique name instead, and a
//! squatter that takes the well-known name afterwards gets nothing.

use porter_dbus::BusConnection;
use zbus::names::{BusName, UniqueName};
use zbus::proxy::{Builder, Defaults};

/// The connection a call is sent to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum Destination {
    /// Whoever owns the interface's well-known name when the call is made.
    #[default]
    WellKnown,
    /// The connection with this unique name (`:1.42`), checked by the caller.
    Owner(UniqueName<'static>),
}

impl Destination {
    /// A proxy of type `P` over `connection`, sent here.
    pub(crate) async fn proxy<P>(&self, connection: &BusConnection) -> zbus::Result<P>
    where
        P: From<zbus::Proxy<'static>> + Defaults,
    {
        self.aim(Builder::<P>::new(connection))?.build().await
    }

    /// `builder`, sent here: unchanged for the well-known name, which is its default.
    pub(crate) fn aim<'a, P>(&self, builder: Builder<'a, P>) -> zbus::Result<Builder<'a, P>> {
        match self {
            Destination::WellKnown => Ok(builder),
            Destination::Owner(owner) => builder.destination(BusName::Unique(owner.clone())),
        }
    }
}
