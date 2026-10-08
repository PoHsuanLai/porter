//! The `Sync1` object and the tasks beside it: the roster of callers and the signal relay.

use super::errors::RefusedError;
use super::hub::{DatasetName, Event, Hub};
use super::resolve::{ConfirmError, ConflictNumber, How, SettleError};
use super::status::{conflict_details, held_details, progress_details, status_details};
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

    /// Sends `event` to each connection that is told of it (see [`Hub::hears`]).
    async fn relay(&self, event: &Event) {
        let names: Vec<String> = held(&self.roster)
            .iter()
            .filter(|(_, caller)| self.hub.hears(caller, event))
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
            Event::Held { dataset, held } => {
                let details = held_details(dataset, held.as_ref());
                SyncObject::<C>::needs_confirmation(&emitter, &dataset.to_string(), details).await
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

    /// Settles one stored conflict of `dataset` (`<account>/<dataset>`). `conflict` is the number
    /// the `Conflict` signal carried (its `number` key); `how` is `keep_local` (upload the local
    /// content over the replica's version) or `keep_remote` (take the replica's version and drop
    /// the local change), and the next cycle does it. Only the dataset's owning app may call it.
    /// Errors: `InvalidArgs` for any other `how`; `org.quire.Accounts1.Error.NoFittingAccount`
    /// when the caller sees no such dataset; `org.quire.Accounts1.Error.Denied` when it sees the
    /// dataset but does not own it; `org.quire.Sync1.Error.NoSuchConflict` when the conflict is
    /// unknown or already settled.
    async fn resolve(
        &self,
        #[zbus(header)] header: Header<'_>,
        dataset: String,
        conflict: i64,
        how: String,
    ) -> Result<(), RefusedError> {
        let caller = self.0.identify(&header).await?;
        // The words first: a malformed call is the caller's to fix whatever it may see.
        let how = how.parse::<How>().map_err(RefusedError::invalid)?;
        let name = self.0.visible(&caller, &dataset)?;
        self.0
            .hub
            .settle(&caller, &name, ConflictNumber(conflict), how)
            .await
            .map_err(|error| match error {
                SettleError::NoSuchDataset => RefusedError::of(Refusal::NoFittingAccount),
                SettleError::NotOwner => RefusedError::of(Refusal::Denied),
                SettleError::NoSuchConflict => RefusedError::no_such_conflict(),
                SettleError::Failed(why) => RefusedError::failed(why),
            })
    }

    /// Lets one discard through for `dataset` (`<account>/<dataset>`) while it is held: the
    /// replica's listing lacks all, or most, of what the dataset holds (`Status` carries
    /// `needs_confirmation`, `NeedsConfirmation` announces it) and nothing was removed here. The
    /// next cycle runs at once and removes those items; the hold is cleared when it ends.
    /// Anyone who may pause the dataset may call it (its owning app, Settings, the porter
    /// daemons). Keeping is not calling it: the dataset stays held and nothing is removed,
    /// every cycle asking the replica for its listing again. Errors:
    /// `org.quire.Accounts1.Error.NoFittingAccount` when the caller sees no such dataset;
    /// `org.quire.Sync1.Error.NothingHeld` when nothing is held (never, or already confirmed).
    async fn confirm_discard(
        &self,
        #[zbus(header)] header: Header<'_>,
        dataset: String,
    ) -> Result<(), RefusedError> {
        let caller = self.0.identify(&header).await?;
        let name = self.0.visible(&caller, &dataset)?;
        self.0
            .hub
            .confirm_discard(&caller, &name)
            .await
            .map_err(|error| match error {
                ConfirmError::NoSuchDataset => RefusedError::of(Refusal::NoFittingAccount),
                ConfirmError::NothingHeld => RefusedError::nothing_held(),
            })
    }

    /// Joins the caller to the connections syncd tells, without asking for any data: any
    /// identified caller may call it. A connection is told only after it has called syncd, so
    /// the shell calls this once at start. The shell (`CallerRole::SheetHost`) is then told
    /// `NeedsConfirmation` for every dataset, the holds already on at once, and nothing else of
    /// a dataset it does not own. Errors: `AccessDenied` for a sender syncd does not know.
    async fn watch(&self, #[zbus(header)] header: Header<'_>) -> Result<(), RefusedError> {
        let caller = self.0.identify(&header).await?;
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?;
        for (dataset, mass) in self.0.hub.holds_for(&caller) {
            let hold = Event::Held {
                dataset,
                held: Some(mass),
            };
            // A signal that cannot be sent is a connection that has gone.
            let _ = self.0.tell(sender.as_str(), &hold).await;
        }
        Ok(())
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

    /// A hold started or ended. The details are those of `Status`'s `needs_confirmation` key
    /// (`discard` and `held`, both `t`) plus `account` (`s`, the account's object path under
    /// `/org/quire/Accounts1/account/`) when the dataset is now held, and empty when the hold
    /// ended (confirmed and done, or the replica listed its items again).
    #[zbus(signal)]
    async fn needs_confirmation(
        emitter: &SignalEmitter<'_>,
        dataset: &str,
        held: Details,
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
