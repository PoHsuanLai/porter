//! The scheduler (design/31 §6.1 "Scheduler"), pure: the clock, the network and each dataset's
//! history are inputs, wake-ups are output. One scheduler per syncd; the driver
//! (`crate::driver`) sleeps until what this says and asks again when an input changes.
//!
//! - Push where a replica declares it: a push signal wakes the dataset after a short batching
//!   window (a burst of signals is one cycle); a slow safety poll still runs.
//! - Else poll: the interval doubles with every cycle that found nothing (up to a cap) and
//!   resets when something changed; a failed cycle backs off exponentially, never sooner than
//!   the replica's `Retry-After`. Delays carry equal jitter from a seeded source.
//! - Wake-ups of different datasets are batched (`coalesce`) so the radio wakes once.
//! - Paused: by the user, offline, or on a metered network when the setting says so.

mod backoff;

pub use backoff::{Jitter, exponential, jittered};

use porter_core::UnixSeconds;
use porter_core::capability::Delta;

/// How the machine is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Network {
    /// No connection.
    Offline,
    /// A connection without a data cap.
    Unmetered,
    /// A connection the user pays for by the byte (the NetworkManager `metered` flag).
    Metered,
}

/// What a metered network does to syncing (a setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeteredPolicy {
    /// Nothing syncs on a metered network.
    Pause,
    /// It syncs as on any other.
    Allow,
}

/// Whether the user stopped this dataset (`Sync1.Pause`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Pausing {
    /// Syncing.
    #[default]
    Running,
    /// Stopped by the user until `Resume`.
    Paused,
}

/// The scheduler's settings, all in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// The first poll interval, and the failure backoff's base.
    pub poll_base: u32,
    /// The longest poll interval, idle or failing.
    pub poll_max: u32,
    /// How long a push signal waits for others before the cycle runs.
    pub push_window: u32,
    /// Wake-ups within this long of the first are batched together.
    pub batch_window: u32,
    /// What a metered network does.
    pub metered: MeteredPolicy,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_base: 60,
            poll_max: 6 * 3600,
            push_window: 2,
            batch_window: 30,
            metered: MeteredPolicy::Pause,
        }
    }
}

/// How the last cycle went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Last {
    /// No cycle yet.
    #[default]
    Never,
    /// A cycle finished at this time; `idle` counts the consecutive cycles that changed nothing
    /// (including this one when it was one).
    Finished {
        /// When it finished.
        at: UnixSeconds,
        /// Consecutive cycles that found nothing.
        idle: u32,
    },
    /// A cycle failed at this time; `failures` counts consecutive failures (including this one)
    /// and `retry_after` is what the replica asked for.
    Failed {
        /// When it failed.
        at: UnixSeconds,
        /// Consecutive failures.
        failures: u32,
        /// The replica's `Retry-After`, seconds.
        retry_after: u32,
    },
}

/// Where a push signal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PushSignal {
    /// None waiting.
    #[default]
    Quiet,
    /// The first of a burst arrived at this time.
    Since(UnixSeconds),
}

/// Everything the scheduler knows about one dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inputs {
    /// What the replica declares (`StorageCap.delta`).
    pub delta: Delta,
    /// How the last cycle went.
    pub last: Last,
    /// A push signal waiting.
    pub push: PushSignal,
    /// The user's pause.
    pub pausing: Pausing,
    /// The network now.
    pub network: Network,
    /// The settings.
    pub settings: Settings,
}

/// Why nothing is scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hold {
    /// The user paused it.
    UserPaused,
    /// There is no network.
    Offline,
    /// The network is metered and the setting says to wait.
    Metered,
}

/// What to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// Run a cycle now.
    Now,
    /// Run a cycle at this time.
    At(UnixSeconds),
    /// Do nothing until an input changes.
    Hold(Hold),
}

/// The next wake-up of one dataset at `now`.
pub fn next_wake(inputs: &Inputs, now: UnixSeconds, source: &mut Jitter) -> Wake {
    if inputs.pausing == Pausing::Paused {
        return Wake::Hold(Hold::UserPaused);
    }
    match (inputs.network, inputs.settings.metered) {
        (Network::Offline, _) => return Wake::Hold(Hold::Offline),
        (Network::Metered, MeteredPolicy::Pause) => return Wake::Hold(Hold::Metered),
        _ => {}
    }
    let settings = &inputs.settings;
    let after =
        |at: UnixSeconds, seconds: u32| UnixSeconds(at.0.saturating_add(i64::from(seconds)));
    let due = match inputs.last {
        Last::Never => return Wake::Now,
        Last::Failed {
            at,
            failures,
            retry_after,
        } => {
            let backoff = exponential(
                settings.poll_base,
                failures.saturating_sub(1),
                settings.poll_max,
            );
            after(at, jittered(backoff, source).max(retry_after))
        }
        Last::Finished { at, idle } => {
            let interval = match inputs.delta {
                Delta::Push => settings.poll_max,
                Delta::Poll | Delta::None => exponential(
                    settings.poll_base,
                    idle.saturating_sub(1),
                    settings.poll_max,
                ),
            };
            let poll = after(at, jittered(interval, source));
            match (inputs.delta, inputs.push) {
                (Delta::Push, PushSignal::Since(first)) => {
                    poll.min(after(first, settings.push_window))
                }
                _ => poll,
            }
        }
    };
    if due <= now { Wake::Now } else { Wake::At(due) }
}

/// Batches wake-ups: sorted by time, each group starts at its earliest and takes every wake
/// within `window` seconds of it, and fires at the group's latest time, so nothing runs before
/// its own time and one wake-up serves the group.
pub fn coalesce<K: Clone>(wakes: &[(K, UnixSeconds)], window: u32) -> Vec<(UnixSeconds, Vec<K>)> {
    let mut sorted: Vec<&(K, UnixSeconds)> = wakes.iter().collect();
    sorted.sort_by_key(|(_, at)| *at);
    let mut groups: Vec<(UnixSeconds, UnixSeconds, Vec<K>)> = Vec::new();
    for (key, at) in sorted {
        match groups.last_mut() {
            Some((first, latest, keys)) if at.0 - first.0 <= i64::from(window) => {
                *latest = *at;
                keys.push(key.clone());
            }
            _ => groups.push((*at, *at, vec![key.clone()])),
        }
    }
    groups
        .into_iter()
        .map(|(_, latest, keys)| (latest, keys))
        .collect()
}

#[cfg(test)]
mod tests;
