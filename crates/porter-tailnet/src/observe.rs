//! Following this computer's Tailscale: who this computer is on the network, while Tailscale is
//! running and signed in, and when it is not.
//!
//! Tailscale streams a notice whenever something about the network changes, and each notice
//! makes the observer look at the status again (the stream is only a nudge: its payloads are
//! large and their shape is Tailscale's to change). The stream needs a running Tailscale, and
//! some versions do not announce every change, so a look is also made every so often while the
//! stream is open, and the stream is opened again every so often while it is not. A look that
//! fails for a passing reason (Tailscale slow to answer, an answer that was cut) says nothing
//! about whether it is running, so what was known stays and the look is made again soon: a
//! busy computer does not lose its place on the network.

use crate::identity::Identity;
use porter_tailscale::{LocalApi, Standing, TailscaleError};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// How often the observer looks when the notices are not coming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// How long between two looks while the stream is open and silent.
    pub backstop: Duration,
    /// How long before the stream is opened again when there is none, and before a look is made
    /// again after one that failed for a passing reason.
    pub retry: Duration,
}

impl Timing {
    /// A look every half minute behind the stream, a new try every ten seconds without one.
    pub const fn usual() -> Self {
        Self {
            backstop: Duration::from_secs(30),
            retry: Duration::from_secs(10),
        }
    }
}

/// What this computer is now: its identity while Tailscale is running and signed in, else none.
pub type Seen = Option<Arc<Identity>>;

/// A task that keeps [`Observer::seen`] up to date until it is dropped.
#[derive(Debug)]
pub struct Observer {
    seen: watch::Receiver<Seen>,
    task: JoinHandle<()>,
}

impl Observer {
    /// Starts following the Tailscale that `api` reaches.
    pub fn start(api: LocalApi, timing: Timing) -> Self {
        let (tx, seen) = watch::channel(None);
        let task = tokio::spawn(follow(api, timing, tx));
        Self { seen, task }
    }

    /// What this computer is now; the receiver changes when it does.
    pub fn seen(&self) -> watch::Receiver<Seen> {
        self.seen.clone()
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// What a look found.
enum Found {
    /// Running and signed in.
    Here(Arc<Identity>),
    /// Tailscale says it is not (signed out, off, not running, not letting this program ask).
    Gone,
    /// Nothing settled: starting, or a passing failure. What was known stays.
    Unsure,
}

async fn look(api: &LocalApi) -> Found {
    match api.status().await {
        Ok(status) => match status.standing() {
            Standing::Ready => match Identity::of(&status) {
                Some(identity) => Found::Here(Arc::new(identity)),
                None => Found::Unsure,
            },
            Standing::SignedOut | Standing::Down => Found::Gone,
            Standing::Changing => Found::Unsure,
        },
        Err(
            TailscaleError::NotInstalled | TailscaleError::NotRunning | TailscaleError::Refused,
        ) => Found::Gone,
        Err(_) => Found::Unsure,
    }
}

/// Looks, publishes what was found, and says when to look again soon (a look that settled
/// nothing).
async fn update(api: &LocalApi, tx: &watch::Sender<Seen>, retry: Duration) -> Option<Instant> {
    match look(api).await {
        Found::Here(identity) => {
            tx.send_if_modified(|seen| {
                let same = seen.as_deref() == Some(&*identity);
                if !same {
                    *seen = Some(identity);
                }
                !same
            });
            None
        }
        Found::Gone => {
            tx.send_if_modified(|seen| seen.take().is_some());
            None
        }
        Found::Unsure => Some(Instant::now() + retry),
    }
}

async fn until(due: Option<Instant>) {
    match due {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn follow(api: LocalApi, timing: Timing, tx: watch::Sender<Seen>) {
    loop {
        let stream = api.watch().await;
        let mut again = update(&api, &tx, timing.retry).await;
        if let Ok(mut notices) = stream {
            loop {
                tokio::select! {
                    notice = notices.next() => match notice {
                        Ok(Some(_)) => again = update(&api, &tx, timing.retry).await,
                        // The stream ended or broke: Tailscale stopped. Look once more, then retry.
                        Ok(None) | Err(_) => break,
                    },
                    () = tokio::time::sleep(timing.backstop) => {
                        again = update(&api, &tx, timing.retry).await;
                    }
                    () = until(again) => again = update(&api, &tx, timing.retry).await,
                }
            }
            again = update(&api, &tx, timing.retry).await;
        }
        // No stream (or just ended): wait out the retry, looking again if one was due.
        let wake = Instant::now() + timing.retry;
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(wake) => break,
                () = until(again) => again = update(&api, &tx, timing.retry).await,
            }
        }
    }
}
