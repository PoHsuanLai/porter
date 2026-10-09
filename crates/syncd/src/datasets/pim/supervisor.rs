//! The supervisor: every `rescan` it asks accountd which Calendar and Contacts grants syncd
//! holds and keeps one [`AccountMirrors`] per granted account and kind. A new grant starts
//! mirroring; a grant that is gone (revoked, or the account removed) stops it and deletes the
//! account's mirror. An accountd that cannot be asked changes nothing.

use super::PimKind;
use super::grants::PimGrants;
use super::mirrors::{AccountMirrors, Wiring};
use crate::paths::AccountDir;
use crate::scheduler::next_look;
use porter_client::Transport;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use tokio::task::JoinHandle;

/// How the supervisor behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PimConfig {
    /// How often grants and collections are read again.
    pub rescan: Duration,
}

impl Default for PimConfig {
    fn default() -> Self {
        Self {
            rescan: Duration::from_secs(600),
        }
    }
}

/// Keeps the mirrors of every granted account.
#[derive(Debug)]
pub struct PimSupervisor<T, G> {
    wiring: Wiring<T>,
    grants: G,
    config: PimConfig,
    accounts: BTreeMap<(AccountDir, PimKind), AccountMirrors>,
    silent: BTreeSet<PimKind>,
}

impl<T, G> PimSupervisor<T, G>
where
    T: Transport + 'static,
    G: PimGrants + 'static,
{
    /// A supervisor over `wiring`, asking `grants`.
    pub fn new(wiring: Wiring<T>, grants: G, config: PimConfig) -> Self {
        Self {
            wiring,
            grants,
            config,
            accounts: BTreeMap::new(),
            silent: BTreeSet::new(),
        }
    }

    /// One look: grants, collections, what to start and what to stop. `true` when every step
    /// went through; `false` when accountd could not be asked or a collection could not be
    /// mirrored (the next look is then soon, see [`next_look`]).
    pub async fn tick(&mut self) -> bool {
        let mut whole = true;
        for kind in PimKind::ALL {
            let Ok(candidates) = self.grants.granted(kind).await else {
                eprintln!("syncd: cannot ask for the {kind:?} grants; asking again soon");
                whole = false;
                continue;
            };
            if candidates.is_empty() && self.silent.insert(kind) {
                eprintln!("syncd: no {kind:?} grant, so no {kind:?} is mirrored");
            }
            let mut granted: Vec<AccountDir> = Vec::new();
            for candidate in &candidates {
                let Some(account) =
                    AccountDir::parse(&porter_core::object_segment(&candidate.account))
                else {
                    continue;
                };
                granted.push(account.clone());
                let mirrors = self
                    .accounts
                    .entry((account.clone(), kind))
                    .or_insert_with(|| AccountMirrors::new(account.clone(), kind));
                if let Err(why) = mirrors.refresh(&self.wiring, candidate).await {
                    eprintln!(
                        "syncd: cannot mirror {kind:?} of {account}: {why}; trying again soon"
                    );
                    whole = false;
                }
            }
            let stale: Vec<(AccountDir, PimKind)> = self
                .accounts
                .keys()
                .filter(|(account, k)| *k == kind && !granted.contains(account))
                .cloned()
                .collect();
            for key in stale {
                if let Some(mut mirrors) = self.accounts.remove(&key) {
                    mirrors.retire_all(&self.wiring).await;
                }
            }
        }
        whole
    }

    /// Runs `tick` every `rescan`, for as long as the task lives; after a look that could not
    /// do all it meant to, again soon and then less often (a passing error is never a stop
    /// until the next rescan).
    pub fn spawn(mut self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut failures = 0u32;
            loop {
                failures = if self.tick().await {
                    0
                } else {
                    failures.saturating_add(1)
                };
                tokio::time::sleep(next_look(self.config.rescan, failures)).await;
            }
        })
    }
}
