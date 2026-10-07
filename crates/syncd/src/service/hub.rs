//! What syncd shares between the engines and the bus: each running dataset's name, who may see
//! it, its latest status and its pause switch. An engine's driver holds a [`Handle`] and
//! publishes; the `Sync1` object reads. Nothing here is async or does I/O.

use super::resolve::{ConflictNumber, How, Settle, SettleError};
use crate::dataset::DatasetId;
use crate::paths::AccountDir;
use crate::scheduler::Pausing;
use porter_core::AppName;
use porter_dbus::{Caller, CallerRole};
use porter_sync::{Quota, StoredConflict};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

/// A dataset of an account as `Sync1` names it: `<account>/<dataset>`, the account's object
/// path segment and the dataset slug.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DatasetName {
    /// The account.
    pub account: AccountDir,
    /// The dataset.
    pub dataset: DatasetId,
}

impl DatasetName {
    /// The name `text` spells, if it is one.
    pub fn parse(text: &str) -> Option<Self> {
        let (account, dataset) = text.split_once('/')?;
        Some(Self {
            account: AccountDir::parse(account)?,
            dataset: DatasetId::parse(dataset)?,
        })
    }
}

impl std::fmt::Display for DatasetName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.account, self.dataset)
    }
}

/// Who may see a dataset: the apps that own it, besides Settings (which shows and pauses
/// everything) and the porter daemons.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Access {
    /// The owning apps.
    pub owners: BTreeSet<AppName>,
}

impl Access {
    /// Whether `caller` sees the dataset.
    pub fn admits(&self, caller: &Caller) -> bool {
        match caller.role {
            CallerRole::Settings | CallerRole::PorterDaemon => true,
            CallerRole::App | CallerRole::SheetHost => self.owners.contains(&caller.app.name),
            CallerRole::Agent | CallerRole::Cua | CallerRole::AgentLauncher => false,
        }
    }
}

impl Access {
    /// Whether `caller` is an app that owns the dataset (Settings and the porter daemons see it
    /// but do not own it).
    pub fn owned_by(&self, caller: &Caller) -> bool {
        match caller.role {
            CallerRole::App | CallerRole::SheetHost => self.owners.contains(&caller.app.name),
            CallerRole::Settings
            | CallerRole::PorterDaemon
            | CallerRole::Agent
            | CallerRole::Cua
            | CallerRole::AgentLauncher => false,
        }
    }
}

/// What `Sync1.Status` reports of one dataset.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusSnapshot {
    /// Seconds since the anchor was stored; absent before the first cycle.
    pub anchor_age: Option<i64>,
    /// Items with work left.
    pub pending: u64,
    /// Unsettled conflicts.
    pub conflicts: u64,
    /// The user's pause.
    pub pausing: Pausing,
    /// The replica's quota, when it reports one.
    pub quota: Option<Quota>,
}

/// What a running dataset tells watchers: transfer progress, or a conflict stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Counts of one finished cycle.
    Progress {
        /// The dataset.
        dataset: DatasetName,
        /// Items stored locally.
        fetched: u64,
        /// Items uploaded.
        uploaded: u64,
    },
    /// A conflict was stored.
    Conflict {
        /// The dataset.
        dataset: DatasetName,
        /// The conflict.
        conflict: Box<StoredConflict>,
    },
}

impl Event {
    /// The dataset it is about.
    pub fn dataset(&self) -> &DatasetName {
        match self {
            Event::Progress { dataset, .. } | Event::Conflict { dataset, .. } => dataset,
        }
    }
}

#[derive(Debug)]
struct Entry {
    access: Access,
    status: StatusSnapshot,
    pausing: watch::Sender<Pausing>,
    settling: mpsc::Sender<Settle>,
    cycling: Cycling,
}

/// Requests waiting for a driver at most (a driver answers between cycles).
const SETTLES_QUEUED: usize = 8;

/// What woke a driver that was waiting on its handle.
#[derive(Debug)]
pub enum Nudge {
    /// The pause switch changed.
    Pause,
    /// The owning app asked to settle a conflict.
    Settle(Settle),
    /// The dataset was dropped from the hub.
    Dropped,
}

/// Held by a dataset's engine for the length of one cycle, so whoever stops the dataset can wait
/// until no cycle is writing its files any more.
#[derive(Debug, Clone, Default)]
pub struct Cycling(Arc<tokio::sync::Mutex<()>>);

/// How long a stop waits for a cycle in flight (a fetch that hangs on the network) before it
/// goes on without it.
const STOP_WITHIN: std::time::Duration = std::time::Duration::from_secs(30);

impl Cycling {
    /// Waits until no cycle holds it, at most [`STOP_WITHIN`]; whether it was free in time.
    async fn finished(&self) -> bool {
        tokio::time::timeout(STOP_WITHIN, self.0.lock())
            .await
            .is_ok()
    }
}

#[derive(Debug)]
struct Inner {
    datasets: Mutex<BTreeMap<DatasetName, Entry>>,
    events: broadcast::Sender<Event>,
}

/// The shared registry of running datasets.
#[derive(Debug, Clone)]
pub struct Hub(Arc<Inner>);

impl Default for Hub {
    fn default() -> Self {
        Self(Arc::new(Inner {
            datasets: Mutex::default(),
            events: broadcast::channel(256).0,
        }))
    }
}

impl Hub {
    fn datasets(&self) -> MutexGuard<'_, BTreeMap<DatasetName, Entry>> {
        // Every critical section is a plain map update.
        self.0
            .datasets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers a running dataset (replacing one of the same name) and returns the engine's end.
    pub fn register(&self, name: DatasetName, access: Access) -> Handle {
        let (pausing, watching) = watch::channel(Pausing::Running);
        let cycling = Cycling::default();
        let (settling, settles) = mpsc::channel(SETTLES_QUEUED);
        self.datasets().insert(
            name.clone(),
            Entry {
                access,
                status: StatusSnapshot::default(),
                pausing,
                settling,
                cycling: cycling.clone(),
            },
        );
        Handle {
            hub: self.clone(),
            name,
            pausing: watching,
            settles,
            cycling,
        }
    }

    /// Settles conflict `number` of `name` for `caller`, as the dataset's driver does between
    /// cycles: the owning app only. A dataset the caller may not see is [`SettleError::NoSuchDataset`],
    /// as a missing one is.
    pub async fn settle(
        &self,
        caller: &Caller,
        name: &DatasetName,
        number: ConflictNumber,
        how: How,
    ) -> Result<(), SettleError> {
        let queue = {
            let datasets = self.datasets();
            let entry = datasets
                .get(name)
                .filter(|entry| entry.access.admits(caller))
                .ok_or(SettleError::NoSuchDataset)?;
            if !entry.access.owned_by(caller) {
                return Err(SettleError::NotOwner);
            }
            entry.settling.clone()
        };
        let (reply, answer) = oneshot::channel();
        queue
            .send(Settle { number, how, reply })
            .await
            .map_err(|_| SettleError::NoSuchDataset)?;
        answer.await.unwrap_or(Err(SettleError::NoSuchDataset))
    }

    /// The names `caller` may see.
    pub fn names_for(&self, caller: &Caller) -> Vec<String> {
        self.datasets()
            .iter()
            .filter(|(_, entry)| entry.access.admits(caller))
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// Whether `caller` sees `name`.
    pub fn sees(&self, caller: &Caller, name: &DatasetName) -> bool {
        self.datasets()
            .get(name)
            .is_some_and(|entry| entry.access.admits(caller))
    }

    /// The status of `name` if `caller` may see it.
    pub fn status_for(&self, caller: &Caller, name: &DatasetName) -> Option<StatusSnapshot> {
        let datasets = self.datasets();
        let entry = datasets
            .get(name)
            .filter(|entry| entry.access.admits(caller))?;
        Some(StatusSnapshot {
            pausing: *entry.pausing.borrow(),
            ..entry.status.clone()
        })
    }

    /// Pauses or resumes `name` if `caller` may see it; whether there was such a dataset.
    pub fn set_pausing(&self, caller: &Caller, name: &DatasetName, pausing: Pausing) -> bool {
        let datasets = self.datasets();
        let Some(entry) = datasets
            .get(name)
            .filter(|entry| entry.access.admits(caller))
        else {
            return false;
        };
        entry.pausing.send_replace(pausing);
        true
    }

    /// Drops one dataset (its engine stops); whether it was there.
    pub fn forget(&self, name: &DatasetName) -> bool {
        self.datasets().remove(name).is_some()
    }

    /// Drops every dataset of `account` (its engines stop): the names that were running.
    pub fn forget_account(&self, account: &AccountDir) -> Vec<DatasetName> {
        let mut datasets = self.datasets();
        let gone: Vec<DatasetName> = datasets
            .keys()
            .filter(|name| name.account == *account)
            .cloned()
            .collect();
        for name in &gone {
            datasets.remove(name);
        }
        gone
    }

    /// Drops `name` and waits until its engine is between cycles (it stops at the next look), so
    /// nothing writes its files once this returns; whether it was there.
    pub async fn stop(&self, name: &DatasetName) -> bool {
        let entry = self.datasets().remove(name);
        match entry {
            Some(entry) => {
                if !entry.cycling.finished().await {
                    eprintln!("syncd: {name}: a cycle still running after the stop wait");
                }
                true
            }
            None => false,
        }
    }

    /// [`Hub::forget_account`], then waits until none of the account's engines is in a cycle:
    /// once it returns, nothing writes the account's journals or mirrors again.
    pub async fn stop_account(&self, account: &AccountDir) -> Vec<DatasetName> {
        let stopped: Vec<(DatasetName, Cycling)> = {
            let mut datasets = self.datasets();
            let names: Vec<DatasetName> = datasets
                .keys()
                .filter(|name| name.account == *account)
                .cloned()
                .collect();
            names
                .into_iter()
                .filter_map(|name| {
                    let entry = datasets.remove(&name)?;
                    Some((name, entry.cycling))
                })
                .collect()
        };
        for (name, cycling) in &stopped {
            if !cycling.finished().await {
                eprintln!("syncd: {name}: a cycle still running after the stop wait");
            }
        }
        stopped.into_iter().map(|(name, _)| name).collect()
    }

    /// Events as they happen (a lagging receiver loses the oldest).
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.events.subscribe()
    }
}

/// An engine's end of its dataset's entry.
#[derive(Debug)]
pub struct Handle {
    hub: Hub,
    name: DatasetName,
    pausing: watch::Receiver<Pausing>,
    settles: mpsc::Receiver<Settle>,
    cycling: Cycling,
}

impl Handle {
    /// The dataset's name.
    pub fn name(&self) -> &DatasetName {
        &self.name
    }

    /// Publishes the latest status (the pause is the hub's, not the engine's).
    pub fn publish(&self, status: StatusSnapshot) {
        if let Some(entry) = self.hub.datasets().get_mut(&self.name) {
            entry.status = status;
        }
    }

    /// Tells watchers something happened. Nobody listening is not an error.
    pub fn tell(&self, event: Event) {
        let _ = self.hub.0.events.send(event);
    }

    /// The user's pause now.
    pub fn pausing(&self) -> Pausing {
        *self.pausing.borrow()
    }

    /// Waits for the pause switch to change; `false` when the dataset was dropped from the hub
    /// (its account was removed) and the engine should stop.
    pub async fn changed(&mut self) -> bool {
        self.pausing.changed().await.is_ok()
    }

    /// Waits for the pause switch to change or for a request to settle a conflict.
    pub async fn nudged(&mut self) -> Nudge {
        tokio::select! {
            changed = self.pausing.changed() => match changed {
                Ok(()) => Nudge::Pause,
                Err(_) => Nudge::Dropped,
            },
            request = self.settles.recv() => match request {
                Some(request) => Nudge::Settle(request),
                None => Nudge::Dropped,
            },
        }
    }

    /// A request to settle a conflict that is already waiting.
    pub fn settle_waiting(&mut self) -> Option<Settle> {
        self.settles.try_recv().ok()
    }

    /// Starts a cycle: holds the dataset's cycle lock until the guard drops. `None` when the
    /// dataset was dropped from the hub (checked under the lock, so a stop that has begun is
    /// never raced by a cycle that starts after it).
    pub async fn begin_cycle(&self) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        let guard = Arc::clone(&self.cycling.0).lock_owned().await;
        self.is_registered().then_some(guard)
    }

    /// Whether the hub still holds this dataset.
    pub fn is_registered(&self) -> bool {
        self.hub.datasets().contains_key(&self.name)
    }
}

#[cfg(test)]
#[path = "hub_tests.rs"]
mod tests;
