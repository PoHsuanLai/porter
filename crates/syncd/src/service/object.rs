//! The `Sync1` object and the tasks beside it: the roster of callers and the signal relay.

use super::errors::RefusedError;
use super::hub::{DatasetName, Event, Hub};
use super::status::{conflict_details, progress_details, status_details};
use crate::scheduler::Pausing;
use porter_core::wire::Refusal;
use porter_dbus::{Caller, Callers, Details, SYNC_BUS, SYNC_PATH};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use zbus::Connection;
use zbus::export::futures_core::Stream;
use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;

/// What every call needs.
#[derive(Debug)]
struct Core<C> {
    hub: Hub,
    callers: C,
    connection: Connection,
    /// The connections that have called, by unique name: who a unicast signal can reach.
    roster: Mutex<BTreeMap<String, Caller>>,
}

fn held<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Every critical section is a plain map update.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<C: Callers> Core<C> {
    /// Who the sender is; an unknown sender is `AccessDenied`. The caller joins the roster.
    async fn identify(&self, header: &Header<'_>) -> Result<Caller, RefusedError> {
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?;
        let caller = self
            .callers
            .caller_of(sender.as_str())
            .await
            .ok_or_else(|| RefusedError::access_denied("syncd does not know this caller"))?;
        held(&self.roster).insert(sender.to_string(), caller.clone());
        Ok(caller)
    }

    /// The dataset `text` names, for a caller that may see it.
    fn visible(&self, caller: &Caller, text: &str) -> Result<DatasetName, RefusedError> {
        let name = DatasetName::parse(text)
            .ok_or_else(|| RefusedError::invalid(format!("`{text}` is not <account>/<dataset>")))?;
        match self.hub.sees(caller, &name) {
            true => Ok(name),
            false => Err(RefusedError::of(Refusal::NoFittingAccount)),
        }
    }

    async fn pausing(
        &self,
        header: &Header<'_>,
        dataset: &str,
        pausing: Pausing,
    ) -> Result<(), RefusedError> {
        let caller = self.identify(header).await?;
        let name = self.visible(&caller, dataset)?;
        self.hub.set_pausing(&caller, &name, pausing);
        Ok(())
    }

    /// Sends `event` to each connection that may see its dataset.
    async fn relay(&self, event: &Event) {
        let names: Vec<String> = held(&self.roster)
            .iter()
            .filter(|(_, caller)| self.hub.sees(caller, event.dataset()))
            .map(|(name, _)| name.clone())
            .collect();
        for name in names {
            let _ = self.tell(&name, event).await;
        }
    }

    async fn tell(&self, name: &str, event: &Event) -> zbus::Result<()> {
        let emitter = SignalEmitter::new(&self.connection, SYNC_PATH)?
            .set_destination(BusName::try_from(name.to_owned())?);
        match event {
            Event::Progress {
                dataset,
                fetched,
                uploaded,
            } => {
                let details = progress_details(*fetched, *uploaded);
                SyncObject::<C>::progress(&emitter, &dataset.to_string(), details).await
            }
            Event::Conflict { dataset, conflict } => {
                let details = conflict_details(conflict);
                SyncObject::<C>::conflict(&emitter, &dataset.to_string(), details).await
            }
        }
    }
}

/// The interface object.
#[derive(Debug)]
struct SyncObject<C>(Arc<Core<C>>);

#[zbus::interface(name = "org.quire.Sync1")]
impl<C: Callers> SyncObject<C> {
    async fn datasets(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<Vec<String>, RefusedError> {
        let caller = self.0.identify(&header).await?;
        Ok(self.0.hub.names_for(&caller))
    }

    async fn status(
        &self,
        #[zbus(header)] header: Header<'_>,
        dataset: String,
    ) -> Result<Details, RefusedError> {
        let caller = self.0.identify(&header).await?;
        let name = self.0.visible(&caller, &dataset)?;
        self.0
            .hub
            .status_for(&caller, &name)
            .map(|status| status_details(&status))
            .ok_or_else(|| RefusedError::of(Refusal::NoFittingAccount))
    }

    async fn pause(
        &self,
        #[zbus(header)] header: Header<'_>,
        dataset: String,
    ) -> Result<(), RefusedError> {
        self.0.pausing(&header, &dataset, Pausing::Paused).await
    }

    async fn resume(
        &self,
        #[zbus(header)] header: Header<'_>,
        dataset: String,
    ) -> Result<(), RefusedError> {
        self.0.pausing(&header, &dataset, Pausing::Running).await
    }

    #[zbus(signal)]
    async fn progress(
        emitter: &SignalEmitter<'_>,
        dataset: &str,
        progress: Details,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn conflict(
        emitter: &SignalEmitter<'_>,
        dataset: &str,
        conflict: Details,
    ) -> zbus::Result<()>;
}

/// Serves `org.quire.Sync1` on `connection` over `hub`, with `callers` saying who calls.
pub async fn serve<C: Callers>(connection: &Connection, hub: Hub, callers: C) -> zbus::Result<()> {
    let core = Arc::new(Core {
        hub,
        callers,
        connection: connection.clone(),
        roster: Mutex::new(BTreeMap::new()),
    });
    connection
        .object_server()
        .at(SYNC_PATH, SyncObject(Arc::clone(&core)))
        .await?;
    let mut owners = DBusProxy::new(connection)
        .await?
        .receive_name_owner_changed()
        .await?;
    let weak = Arc::downgrade(&core);
    tokio::spawn(async move {
        while let Some(signal) =
            std::future::poll_fn(|cx| Pin::new(&mut owners).poll_next(cx)).await
        {
            let Some(core) = weak.upgrade() else { return };
            if let Ok(args) = signal.args()
                && args.new_owner().is_none()
            {
                held(&core.roster).remove(args.name().as_str());
            }
        }
    });
    let mut events = core.hub.subscribe();
    let weak = Arc::downgrade(&core);
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    let Some(core) = weak.upgrade() else { return };
                    core.relay(&event).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    connection.request_name(SYNC_BUS).await?;
    Ok(())
}
