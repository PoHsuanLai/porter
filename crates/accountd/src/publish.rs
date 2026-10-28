//! Telling the clients what changed. Objects that change the registry ask `Core::publish`, which
//! hands the request to the one task started here and waits for its answer: the task registers
//! and removes the `Account` objects and sends each signal to the connections of the roles it
//! concerns, one request at a time.

use crate::account::AccountObject;
use crate::callers::Callers;
use crate::host::Host;
use crate::hub::{Event, events, hears, settings_news};
use crate::manager::Manager;
use crate::state::{Core, held};
use porter_core::AccountId;
use porter_dbus::accounts::AccountServed;
use porter_dbus::accounts::ManagerServed;
use porter_dbus::accounts::{ACCOUNTS_PATH, account_path};
use porter_service::Registry;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use zbus::fdo::Properties;
use zbus::names::{BusName, InterfaceName};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::ObjectPath;

/// Starts the task that answers `Core::publish`. It holds the core weakly, so it ends with it.
pub(crate) fn start<H: Host, C: Callers>(core: &Arc<Core<H, C>>) {
    let (requests, mut asked) = mpsc::unbounded_channel::<oneshot::Sender<()>>();
    let _ = core.publisher.set(requests);
    let core = Arc::downgrade(core);
    tokio::spawn(async move {
        while let Some(done) = asked.recv().await {
            let Some(core) = core.upgrade() else { return };
            core.tell_all().await;
            let _ = done.send(());
        }
    });
}

impl<H: Host, C: Callers> Core<H, C> {
    /// Tells the clients what changed since they were last told: registers and removes the
    /// `Account` objects, and sends each signal to the connections of the roles it concerns
    /// (the shell and Settings, the only ones that read an account).
    async fn tell_all(self: &Arc<Self>) {
        let after = self.host.registry();
        let before = std::mem::replace(&mut *held(&self.published), after.clone());
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
                    let _ = server
                        .remove::<AccountServed<AccountObject<H, C>>, _>(account_path(id))
                        .await;
                }
                _ => {}
            }
        }
        self.announce_states(&list, &before, &after).await;
        self.announce_identities_removed(&before, &after).await;
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
            Event::NeedsReauth(id) => {
                ManagerServed::<Manager<H, C>>::needs_reauth(&emitter, path(id)?).await
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
