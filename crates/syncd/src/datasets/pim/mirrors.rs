//! One account's collections of one kind, kept running: discovery names them, each gets a
//! [`PimMirror`] over its vdir directory, a replica over a relay to its URL, a journal and a
//! driver registered in the hub (so `Sync1` shows `<account>/pim_cal_personal`). A refresh finds
//! new collections, rewrites renamed or recoloured ones, and retires the ones the server no
//! longer has.

use super::discover::{DiscoverError, discover};
use super::mirror::PimMirror;
use super::plan::{Planned, plan};
use super::relay::{pim_http, pim_replica};
use super::{Meta, PimKind};
use crate::clock::SystemClock;
use crate::dataset::DatasetId;
use crate::driver::Driver;
use crate::engine::Engine;
use crate::journal::Journal;
use crate::paths::{AccountDir, Paths};
use crate::scheduler::{Network, Settings};
use crate::service::{Access, DatasetName, Hub};
use porter_client::{Accounts, Transport};
use porter_core::{Candidate, WebUrl};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use storage_webdav::Clock;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

/// What mirrors share: the connection to accountd, the hub they register in, the paths and the
/// scheduler's inputs.
#[derive(Debug)]
pub struct Wiring<T> {
    /// accountd, for the relays.
    pub accounts: Arc<Accounts<T>>,
    /// The running datasets.
    pub hub: Hub,
    /// Where journals and mirrors live.
    pub paths: Paths,
    /// The scheduler's settings for each collection.
    pub settings: Settings,
    /// The network, as the daemon reads it.
    pub network: watch::Receiver<Network>,
    /// The apps that see the datasets in `Sync1` besides Settings and the porter daemons.
    pub owners: Access,
}

/// Why an account's collections could not be refreshed.
#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    /// The candidate lists no server for this kind.
    #[error("the account has no {0:?} server")]
    NoEndpoint(PimKind),
    /// The server's collections could not be read.
    #[error(transparent)]
    Discover(#[from] DiscoverError),
    /// A collection could not be opened.
    #[error("{0}")]
    Open(String),
}

#[derive(Debug)]
struct Running {
    planned: Planned,
    mirror: PimMirror,
    task: JoinHandle<()>,
}

/// The running collections of one account and kind.
#[derive(Debug)]
pub struct AccountMirrors {
    account: AccountDir,
    kind: PimKind,
    running: BTreeMap<DatasetId, Running>,
}

impl AccountMirrors {
    /// Nothing running yet for `account`.
    pub fn new(account: AccountDir, kind: PimKind) -> Self {
        Self {
            account,
            kind,
            running: BTreeMap::new(),
        }
    }

    /// The datasets running now.
    pub fn datasets(&self) -> Vec<DatasetId> {
        self.running.keys().cloned().collect()
    }

    /// Reads the account's collections and brings the running ones to match.
    pub async fn refresh<T: Transport + 'static>(
        &mut self,
        wiring: &Wiring<T>,
        candidate: &Candidate,
    ) -> Result<(), RefreshError> {
        let endpoint = candidate
            .endpoints
            .iter()
            .find(|e| e.family == self.kind.family())
            .or_else(|| candidate.endpoints.first())
            .ok_or(RefreshError::NoEndpoint(self.kind))?;
        let web = WebUrl::try_from(&endpoint.url).map_err(|_| DiscoverError::Unreadable)?;
        let http = pim_http(
            Arc::clone(&wiring.accounts),
            candidate.grant.clone(),
            endpoint.url.clone(),
        );
        let planned = plan(self.kind, discover(&http, &web, self.kind).await?);
        let mut failure = None;
        for next in &planned {
            let meta = Meta {
                displayname: next.found.displayname.clone(),
                color: next.found.color.clone(),
            };
            let kept = match self.running.get_mut(&next.dataset) {
                Some(held)
                    if held.planned.dir == next.dir && held.planned.found.url == next.found.url =>
                {
                    held.planned = next.clone();
                    held.mirror.write_meta(&meta).map_err(|e| e.0)
                }
                Some(_) => Err("two collections want one name".to_owned()),
                None => self
                    .start(wiring, candidate, next, &meta)
                    .map_err(|e| format!("{}: {e}", next.dir)),
            };
            failure = failure.or(kept.err());
        }
        let gone: Vec<DatasetId> = self
            .running
            .keys()
            .filter(|id| !planned.iter().any(|p| p.dataset == **id))
            .cloned()
            .collect();
        for id in gone {
            self.retire(wiring, &id);
        }
        failure.map_or(Ok(()), |why| Err(RefreshError::Open(why)))
    }

    fn start<T: Transport + 'static>(
        &mut self,
        wiring: &Wiring<T>,
        candidate: &Candidate,
        next: &Planned,
        meta: &Meta,
    ) -> Result<(), String> {
        let endpoint = candidate
            .endpoints
            .iter()
            .find(|e| e.family == self.kind.family())
            .or_else(|| candidate.endpoints.first())
            .ok_or("no server")?;
        let journal = Journal::open(&wiring.paths.journal(&self.account, next.dataset.as_str()))
            .map_err(|e| e.to_string())?;
        let root = wiring
            .paths
            .mirrors
            .join(self.account.as_str())
            .join(&next.dir);
        let mirror =
            PimMirror::open(next.dataset.clone(), self.kind, root, &journal).map_err(|e| e.0)?;
        mirror.write_meta(meta).map_err(|e| e.0)?;
        let replica = pim_replica(
            Arc::clone(&wiring.accounts),
            candidate.grant.clone(),
            endpoint.url.clone(),
            &next.found.url,
            Clock::system(),
        );
        let engine = Engine::new(replica, mirror.clone(), journal, SystemClock);
        let name = DatasetName {
            account: self.account.clone(),
            dataset: next.dataset.clone(),
        };
        let handle = wiring.hub.register(name, wiring.owners.clone());
        let seed = u64::from_be_bytes(
            Sha256::digest(next.found.url.as_str().as_bytes())[..8]
                .try_into()
                .unwrap_or([0; 8]),
        );
        let driver = Driver::new(
            engine,
            handle,
            wiring.settings,
            wiring.network.clone(),
            Arc::new(Notify::new()),
            seed,
        );
        self.running.insert(
            next.dataset.clone(),
            Running {
                planned: next.clone(),
                mirror,
                task: tokio::spawn(driver.run()),
            },
        );
        Ok(())
    }

    /// Stops one collection and deletes what it kept: its files, its journal.
    fn retire<T>(&mut self, wiring: &Wiring<T>, id: &DatasetId) {
        let Some(held) = self.running.remove(id) else {
            return;
        };
        held.task.abort();
        wiring.hub.forget(&DatasetName {
            account: self.account.clone(),
            dataset: id.clone(),
        });
        let _ = std::fs::remove_dir_all(held.mirror.root());
        let _ = std::fs::remove_file(wiring.paths.journal(&self.account, id.as_str()));
    }

    /// Stops every collection and deletes what they kept (the grant is gone).
    pub fn retire_all<T>(&mut self, wiring: &Wiring<T>) {
        for id in self.datasets() {
            self.retire(wiring, &id);
        }
    }
}

impl Drop for AccountMirrors {
    fn drop(&mut self) {
        for held in self.running.values() {
            held.task.abort();
        }
    }
}
