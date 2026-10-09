//! `org.quire.Tailnet1` at `/org/quire/Tailnet1`, on accountd's connection: the person's own
//! computers on their Tailscale network (`tailnet` follows Tailscale).
//!
//! - `Machines` is answered to the shell (`SheetHost`), Settings and the terminal (`Terminal`,
//!   temor); anyone else, and a sender accountd does not know, is `AccessDenied`. With no
//!   Tailscale account, or one that is not working, the list is empty, not an error: the
//!   account's own state says why.
//! - `Changed()` goes, with nothing in it, to each connection of those roles that has called
//!   accountd; a listener calls `Machines` again. Changes in a burst are one signal, and there
//!   is at most one a second.

use crate::callers::Callers;
use crate::core::{Core, Host, Standing};
use crate::errors::RefusedError;
use crate::tailnet::machines;
use porter_dbus::{CallerRole, Details, TAILNET_PATH, machine_to_dbus};
use std::sync::Arc;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;

/// Whether a role may read the person's computers: the shell, Settings, the terminal, and the
/// AI broker (`PorterDaemon`), which looks for the computers that lend their models and asks
/// nothing of Tailscale itself for that list.
pub(crate) fn may_read(role: CallerRole) -> bool {
    matches!(
        role,
        CallerRole::SheetHost
            | CallerRole::Settings
            | CallerRole::Terminal
            | CallerRole::PorterDaemon
    )
}

/// The Tailnet object.
#[derive(Debug)]
pub(crate) struct TailnetObject<H, C>(Arc<Core<H, C>>);

impl<H, C> TailnetObject<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

#[zbus::interface(name = "org.quire.Tailnet1")]
impl<H: Host, C: Callers> TailnetObject<H, C> {
    async fn machines(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<Vec<(String, Details)>, RefusedError> {
        let caller = self.0.identify(&header, Standing::Machines).await?;
        if !may_read(caller.role) {
            return Err(RefusedError::access_denied(
                "only the shell, Settings and the terminal may read the person's computers",
            ));
        }
        let Some(watch) = &self.0.tailnet else {
            return Ok(Vec::new());
        };
        Ok(machines(&*self.0.host, &watch.api)
            .await
            .iter()
            .map(machine_to_dbus)
            .collect())
    }

    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Tells every connection that may read the computers, and has called accountd, that they
/// changed.
pub(crate) async fn announce<H: Host, C: Callers>(core: &Arc<Core<H, C>>) {
    for name in core.known_where(|caller| may_read(caller.role)) {
        let Ok(destination) = BusName::try_from(name) else {
            continue;
        };
        let Ok(emitter) = SignalEmitter::new(&core.connection, TAILNET_PATH) else {
            continue;
        };
        let emitter = emitter.set_destination(destination);
        let _ = TailnetObject::<H, C>::changed(&emitter).await;
    }
}
