//! Runs one engine on the scheduler's wake-ups: ask the scheduler what is next, sleep until it,
//! run the cycle, publish the status. Everything that decides is `scheduler` (pure); this is the
//! loop around it. The network is a seam (`watch::Receiver<Network>`; NetworkManager is the
//! daemon's to read, never a test's), a push signal is a `Notify`, the pause switch and the
//! account's removal come through the hub's [`Handle`].

use crate::clock::Clock;
use crate::dataset::Dataset;
use crate::engine::{Engine, Outcome, Report, SyncError};
use crate::scheduler::{Inputs, Jitter, Last, Network, PushSignal, Settings, Wake, next_wake};
use crate::service::{Event, Handle, Nudge, Settle, SettleError, StatusSnapshot};
use porter_sync::{Quota, Replica, StoredConflict};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, watch};

/// One engine, its scheduler state and its ends of the daemon's seams.
#[derive(Debug)]
pub struct Driver<R, D, K> {
    engine: Engine<R, D, K>,
    handle: Handle,
    settings: Settings,
    network: watch::Receiver<Network>,
    push: Arc<Notify>,
    jitter: Jitter,
    last: Last,
    waiting: PushSignal,
    quota: Option<Quota>,
}

impl<R: Replica, D: Dataset, K: Clock> Driver<R, D, K> {
    /// A driver for `engine`, publishing through `handle`, reading the network from `network`,
    /// woken early by `push` where the replica declares push.
    pub fn new(
        engine: Engine<R, D, K>,
        handle: Handle,
        settings: Settings,
        network: watch::Receiver<Network>,
        push: Arc<Notify>,
        seed: u64,
    ) -> Self {
        Self {
            engine,
            handle,
            settings,
            network,
            push,
            jitter: Jitter::seeded(seed),
            last: Last::Never,
            waiting: PushSignal::Quiet,
            quota: None,
        }
    }

    /// Runs until the dataset is dropped from the hub (its account was removed).
    pub async fn run(mut self) {
        while self.handle.is_registered() {
            while let Some(request) = self.handle.settle_waiting() {
                self.settle(request);
            }
            let now = self.engine_now();
            let inputs = Inputs {
                delta: self.engine.replica().features().delta,
                last: self.last,
                push: self.waiting,
                pausing: self.handle.pausing(),
                network: *self.network.borrow(),
                settings: self.settings,
            };
            match next_wake(&inputs, now, &mut self.jitter) {
                Wake::Now => {
                    // Under the dataset's cycle lock: a removal waits for it, so no file is
                    // written once the account's mirror is deleted.
                    let Some(_cycle) = self.handle.begin_cycle().await else {
                        break;
                    };
                    self.cycle().await;
                }
                Wake::At(at) => {
                    let wait = u64::try_from(at.0.saturating_sub(now.0)).unwrap_or(0);
                    self.wait(Some(Duration::from_secs(wait))).await;
                }
                Wake::Hold(_) => self.wait(None).await,
            }
        }
    }

    /// The engine's clock: a cycle's dating and the scheduler's must agree.
    fn engine_now(&self) -> porter_core::UnixSeconds {
        self.engine.now()
    }

    /// Sleeps for `timeout` (forever when `None`) or until something the scheduler reads changes.
    async fn wait(&mut self, timeout: Option<Duration>) {
        let sleep = async {
            match timeout {
                Some(after) => tokio::time::sleep(after).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = sleep => {}
            nudge = self.handle.nudged() => {
                if let Nudge::Settle(request) = nudge {
                    self.settle(request);
                }
            }
            _ = self.network.changed() => {}
            () = self.push.notified() => {
                if self.waiting == PushSignal::Quiet {
                    self.waiting = PushSignal::Since(self.engine_now());
                }
            }
        }
    }

    /// Settles one conflict between cycles (the engine is this driver's alone), answers the
    /// caller and, when it was settled, has the next cycle run at once to do it.
    fn settle(&mut self, request: Settle) {
        let Settle { number, how, reply } = request;
        let answer = match self.engine.resolve(number.0, how.0) {
            Ok(true) => Ok(()),
            Ok(false) => Err(SettleError::NoSuchConflict),
            Err(error) => Err(SettleError::Failed(error.to_string())),
        };
        if answer.is_ok() {
            self.last = Last::Never;
            self.publish(self.engine_now());
        }
        // The caller may have gone: nothing to tell then.
        let _ = reply.send(answer);
    }

    async fn cycle(&mut self) {
        let result = self.engine.sync_once().await;
        let now = self.engine_now();
        self.waiting = PushSignal::Quiet;
        self.last = self.next_last(&result, now);
        if let Ok(report) = &result {
            self.quota = report.quota.or(self.quota);
            self.announce(report);
        }
        self.publish(now);
    }

    fn next_last(&self, result: &Result<Report, SyncError>, now: porter_core::UnixSeconds) -> Last {
        let failed = |retry_after: u32| {
            let failures = match self.last {
                Last::Failed { failures, .. } => failures.saturating_add(1),
                _ => 1,
            };
            Last::Failed {
                at: now,
                failures,
                retry_after,
            }
        };
        match result {
            Ok(report) => match report.outcome {
                Outcome::Retry(after) => failed(after.0),
                // The account must sign in again, or the folder is gone: no point before the
                // slowest poll; the scheduler's backoff keeps asking.
                Outcome::Unauthorized | Outcome::Gone => failed(self.settings.poll_max),
                Outcome::Done | Outcome::QuotaFull => {
                    let quiet = report.fetched
                        + report.uploaded
                        + report.removed
                        + report.discarded
                        + report.conflicts.len()
                        == 0;
                    let idle = match (quiet, self.last) {
                        (true, Last::Finished { idle, .. }) => idle.saturating_add(1),
                        (true, _) => 1,
                        (false, _) => 0,
                    };
                    Last::Finished { at: now, idle }
                }
            },
            Err(_) => failed(0),
        }
    }

    fn announce(&self, report: &Report) {
        let dataset = self.handle.name().clone();
        if report.fetched + report.uploaded > 0 {
            self.handle.tell(Event::Progress {
                dataset: dataset.clone(),
                fetched: report.fetched as u64,
                uploaded: report.uploaded as u64,
            });
        }
        let stored = self.engine.journal().conflicts().unwrap_or_default();
        for conflict in &report.conflicts {
            self.handle.tell(Event::Conflict {
                dataset: dataset.clone(),
                conflict: Box::new(numbered(conflict, &stored)),
            });
        }
    }

    fn publish(&self, now: porter_core::UnixSeconds) {
        let journal = self.engine.journal();
        let anchor_age = journal
            .anchor()
            .ok()
            .flatten()
            .map(|stored| now.0.saturating_sub(stored.at.0).max(0));
        let pending = journal.items().map_or(0, |rows| {
            rows.iter().filter(|row| row.state.is_pending()).count()
        });
        let conflicts = journal.conflicts().map_or(0, |all| all.len());
        self.handle.publish(StatusSnapshot {
            anchor_age,
            pending: pending as u64,
            conflicts: conflicts as u64,
            pausing: self.handle.pausing(),
            quota: self.quota,
        });
    }
}

/// `reported` with the number the journal gave it (the report is made before the row is
/// stored): the newest stored conflict of the same local item and sides, if any.
fn numbered(reported: &StoredConflict, stored: &[StoredConflict]) -> StoredConflict {
    let number = stored
        .iter()
        .rev()
        .find(|row| row.local == reported.local && row.conflict == reported.conflict)
        .and_then(|row| row.number);
    StoredConflict {
        number: number.or(reported.number),
        ..reported.clone()
    }
}
