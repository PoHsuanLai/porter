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

/// Where a request to stop comes from: the signals in the binary, a channel in a program that
/// runs the daemon inside it. Asked twice, the second request hurries the stop.
pub trait Shutdown {
    /// Completes when a stop is asked for. Called again after a stop began, it completes at the
    /// next request; a source that will never ask again never completes.
    fn requested(&mut self) -> impl Future<Output = ()> + Send;
}

impl Shutdown for Signals {
    async fn requested(&mut self) {
        self.next().await;
    }
}

/// A request is a `()` sent on the channel. Every sender dropped is no request, never a
/// completion.
impl Shutdown for tokio::sync::mpsc::UnboundedReceiver<()> {
    async fn requested(&mut self) {
        if self.recv().await.is_none() {
            std::future::pending::<()>().await;
        }
    }
}

/// Waits for the first request (a signal, in the binary), then runs `wind_down` (stop taking
/// work), ends every engine of `engines`, all within `bound`; a second request, or the bound,
/// kills every group and returns.
pub async fn on_signal<S: Shutdown, F: Future<Output = ()>>(
    signals: &mut S,
    engines: &HostCloser,
    wind_down: F,
    bound: Duration,
) -> Ended {
    signals.requested().await;
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
        () = signals.requested() => {
            engines.kill_all();
            Ended::Hurried
        }
    }
}
