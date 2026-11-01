//! Telling the clients what changed. The host sends each change of the registry as it is
//! committed, `Committed { before, after }`, down one ordered channel, and the one task started
//! here takes them in turn: it registers and removes the `Account` objects and sends each signal
//! to the connections of the roles it concerns. Nothing is diffed against a copy held here.

use crate::account::AccountObject;
use crate::callers::Callers;
use crate::host::Host;
use crate::hub::{Event, events, hears, settings_news};
use crate::manager::Manager;
use crate::state::Core;
use porter_core::AccountId;
use porter_dbus::accounts::AccountServed;
use porter_dbus::accounts::ManagerServed;
use porter_dbus::accounts::account_path;
use porter_service_core::Committed;
use porter_service_core::registry::Registry;
use porter_wire::names::ACCOUNTS_PATH;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use zbus::fdo::Properties;
use zbus::names::{BusName, InterfaceName};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::ObjectPath;

/// What the news task is handed, in the order it was sent.
pub(crate) enum Item {
    /// One change of the registry, saved and swapped in.
    Committed(Committed),
    /// Someone waiting until everything sent before it has been told.
    Told(oneshot::Sender<()>),
}

/// Starts the one task that tells the clients, and has `host` send it each change of the registry
/// as it is committed. The task takes the changes one at a time, in the order they were made, and
/// holds the core weakly, so it ends with it.
pub(crate) fn start<H: Host, C: Callers>(
    core: &Arc<Core<H, C>>,
    mut heard: mpsc::UnboundedReceiver<Item>,
) {
    let news = core.news.clone();
    core.host.follow_committed(Box::new(move |committed| {
        let _ = news.send(Item::Committed(committed));
    }));
    let core = Arc::downgrade(core);
    tokio::spawn(async move {
        while let Some(item) = heard.recv().await {
            let Some(core) = core.upgrade() else { return };
            match item {
                Item::Committed(committed) => core.tell_all(committed).await,
                Item::Told(done) => {
                    let _ = done.send(());
                }
            }
        }
    });
}

impl<H: Host, C: Callers> Core<H, C> {
    /// Returns once every change committed before the call has been told: the `Account` objects
    /// registered or removed and the signals sent. A reply that follows its own change waits
    /// here, so a client that hears it finds the news already out. A service that is not being
    /// served yet tells no one.
    pub(crate) async fn told(&self) {
        let (done, finished) = oneshot::channel();
        if self.news.send(Item::Told(done)).is_ok() {
            let _ = finished.await;
        }
    }

    /// Tells the clients what `committed` changed: registers and removes the `Account` objects,
    /// and sends each signal to the connections of the roles it concerns (the shell and
    /// Settings, the only ones that read an account).
    async fn tell_all(self: &Arc<Self>, committed: Committed) {
        let Committed { before, after } = committed;
        let list = events(&before, &after);
        let server = self.connection.object_server();
        for event in &list {
            match event {
                Event::Added(id) => {
                    let _ = server
                        .at(
                            account_path(id),
                            AccountServed(AccountObject::new(Arc::clone(self))),
                        )
                        .await;
                }
                Event::Removed(id) => {
                    // Already gone is as good as removed.
                    let _ = server
                        .remove::<AccountServed<AccountObject<H, C>>, _>(account_path(id))
                        .await;
                }
                _ => {}
            }
        }
        self.announce_states(&list, &before, &after).await;
        self.announce_identities_removed(&before, &after).await;
        // The AI broker asks again, and a credential does not outlive its permit.
        crate::broker::announce(self).await;
        self.sweep().await;
        for event in list {
            let names = self.known_where(|who| hears(who.role, &event));
            for name in names {
                let _ = self.tell(&name, &event).await;
            }
        }
    }

    /// Tells the settings module's listeners (the Settings role is the only one it admits) what
    /// changed in the keys they list, as `Changed(key, value)`:
    ///
    /// - a new state: `accounts.<id>.state` with the state slug, so a sign-in that finishes
    ///   later reaches its pane without a `Set`;
    /// - an account that appeared: its `state` row, and one that went: the same key with
    ///   `removed`, since the module has no signal for "the key set changed" and a pane reads
    ///   the schema again on any `Changed` (detent's `Followed::Changed`);
    /// - a new label: `accounts.<id>.label`.
    async fn announce_states(&self, list: &[Event], before: &Registry, after: &Registry) {
        let Some(module) = self.settings.get() else {
            return;
        };
        for (path, value) in settings_news(list, before, after) {
            let _ = module.changed(&path, &value).await;
        }
    }

    /// Sends `event` to the connection `name` alone.
    async fn tell(&self, name: &str, event: &Event) -> zbus::Result<()> {
        let emitter = SignalEmitter::new(&self.connection, ACCOUNTS_PATH)?
            .set_destination(BusName::try_from(name.to_owned())?);
        let path = |id: &AccountId| ObjectPath::try_from(account_path(id));
        match event {
            Event::Added(id) => {
                ManagerServed::<Manager<H, C>>::account_added(&emitter, path(id)?).await
            }
            Event::Removed(id) => {
                ManagerServed::<Manager<H, C>>::account_removed(&emitter, path(id)?).await
            }
            Event::StateChanged(id) => {
                // The new value is read by the receiver, which `State` lets only the shell and
                // Settings do.
                let emitter = SignalEmitter::new(&self.connection, account_path(id))?
                    .set_destination(BusName::try_from(name.to_owned())?);
                Properties::properties_changed(
                    &emitter,
                    InterfaceName::try_from("org.quire.Accounts1.Account")?,
                    HashMap::new(),
                    Cow::Borrowed(&["State"]),
                )
                .await
            }
        }
    }
}
