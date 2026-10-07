//! Stopping inferd on `SIGTERM` or `SIGINT` (`systemctl --user stop inferd` sends `SIGTERM`
//! first). The daemon stops taking work (the bus name is released), ends every engine through the
//! host's group stop (`SIGTERM` to the group, [`crate::hosts::STOP_GRACE`], `SIGKILL`) so no
//! grandchild such as vLLM's `EngineCore` is left holding GPU memory, and exits 0.
//!
//! The whole of it is bounded by [`BOUND`], 15 s, under `TimeoutStopSec=20` in
//! `dist/inferd.service`: past it every group is killed at once and the exit is an error. A second
//! signal while shutting down does the same without waiting.

use crate::hosts::HostCloser;
use std::future::Future;
use std::time::Duration;
use tokio::signal::unix::{Signal, SignalKind, signal};

/// The longest a graceful shutdown may take. Two stop graces (the leader, then the rest of its
/// group: 10 s) and the bus, with margin, and 5 s under the unit's `TimeoutStopSec=20`.
pub const BOUND: Duration = Duration::from_secs(15);

/// How a shutdown ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// Everything stopped by itself within the bound.
    Done,
    /// The bound ran out: every group was killed.
    TimedOut,
    /// A second signal came: every group was killed at once.
    Hurried,
}

/// `SIGTERM` and `SIGINT`, listened for from the moment this is made (a signal that comes while
/// the daemon is still starting is kept).
#[derive(Debug)]
pub struct Signals {
    term: Signal,
    int: Signal,
}

impl Signals {
    /// Starts listening.
    pub fn listen() -> std::io::Result<Self> {
        Ok(Self {
            term: signal(SignalKind::terminate())?,
            int: signal(SignalKind::interrupt())?,
        })
    }

    /// The next signal of either kind.
    pub async fn next(&mut self) -> Option<SignalKind> {
        tokio::select! {
            got = self.term.recv() => got.map(|()| SignalKind::terminate()),
            got = self.int.recv() => got.map(|()| SignalKind::interrupt()),
        }
    }
}

/// Waits for the first signal, then runs `wind_down` (stop taking work), ends every engine of
/// `engines`, all within `bound`; a second signal, or the bound, kills every group and returns.
pub async fn on_signal<F: Future<Output = ()>>(
    signals: &mut Signals,
    engines: &HostCloser,
    wind_down: F,
    bound: Duration,
) -> Ended {
    signals.next().await;
    let orderly = async {
        wind_down.await;
        engines.end_all().await;
    };
    tokio::select! {
        done = tokio::time::timeout(bound, orderly) => {
            if done.is_ok() {
                Ended::Done
            } else {
                engines.kill_all();
                Ended::TimedOut
            }
        }
        _ = signals.next() => {
            engines.kill_all();
            Ended::Hurried
        }
    }
}
