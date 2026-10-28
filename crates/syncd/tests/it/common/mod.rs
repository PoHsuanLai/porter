//! The rig every bus test of syncd uses: a private bus, callers named by unique name, clients.
//! Nothing here reaches the real session bus, `~/.config` or the network.
#![allow(dead_code)]

pub mod bus;
pub mod hosted;

use bus::PrivateBus;
use porter_callers::{Caller, CallerRole, Callers};
use porter_core::{AppId, AppName, Isolation};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

pub const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";
pub const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";
pub const NO_FITTING: &str = "org.quire.Accounts1.Error.NoFittingAccount";

/// Whether a rig's account holds syncd's Photos grant, which the account's Photos row in Settings
/// makes and takes back: the only thing that runs Photos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotosGrant {
    /// It holds one: Photos runs.
    Granted,
    /// It holds none: Photos runs nothing.
    NotGranted,
}

/// The D-Bus error name of a failed call.
pub fn error_name(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        zbus::Error::FDO(fdo) => {
            use zbus::DBusError;
            fdo.name().to_string()
        }
        other => format!("{other:?}"),
    }
}

pub fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("app name"),
        isolation: Isolation::Flatpak,
    }
}

pub fn caller(name: &str, role: CallerRole) -> Caller {
    Caller {
        app: app(name),
        role,
    }
}

/// Callers by unique name, as a host that knows its clients.
#[derive(Debug, Default, Clone)]
pub struct Known(Arc<Mutex<BTreeMap<String, Caller>>>);

impl Known {
    pub fn introduce(&self, unique_name: &str, caller: Caller) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(unique_name.to_owned(), caller);
    }
}

impl Callers for Known {
    async fn caller_of(&self, sender: &str) -> Option<Caller> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(sender)
            .cloned()
    }
}

/// A new client connection on the bus, introduced as `name` in `role`.
pub async fn client(
    bus: &PrivateBus,
    known: &Known,
    name: &str,
    role: CallerRole,
) -> zbus::Connection {
    let connection = bus.connect().await;
    let unique = connection.unique_name().expect("unique name").to_string();
    known.introduce(&unique, caller(name, role));
    connection
}

/// Has every running, not paused dataset of `hub` sync now ("Sync now") and waits until each has
/// finished a cycle that began after this call, so a cycle that saw everything done before it
/// (an edit, a remote change) has happened, however loaded the machine is. A dataset that is
/// paused, or not running, is not waited for. This is the wait for "a cycle passed"; no poll
/// interval, no sleep.
pub async fn a_cycle_of_each(hub: &syncd::service::Hub) {
    let mut asked = Vec::new();
    for name in hub.names() {
        let Some(cycles) = hub.cycles(&name) else {
            continue;
        };
        if hub.sync_now(&name).is_ok() {
            asked.push((name, cycles.begun));
        }
    }
    for (name, begun) in asked {
        eventually("a cycle that began after the call ends", || {
            hub.cycles(&name).is_none_or(|cycles| cycles.ended > begun)
        })
        .await;
    }
}

/// How long the rigs take for what a daemon would take `ms` for: their scheduler seconds are
/// `Settings::quick().time_scale` to the real one. Only for waits that "nothing happens" in, and
/// nothing is running to ask (use [`a_cycle_of_each`] where a dataset runs): a loaded machine
/// runs fewer polls in that time, which makes the proof thinner, never the test flaky. A wait
/// for something to happen is [`eventually`].
pub fn poll_time(ms: u64) -> std::time::Duration {
    std::time::Duration::from_millis(ms) / syncd::scheduler::Settings::quick().time_scale
}

/// Polls `check` every 20 ms for up to [`test_kit::GENEROUS`] by the clock (not by counting
/// sleeps, which a loaded machine stretches several times over): a passing check returns at
/// once, and a loaded machine (a load average over a hundred) needs far more than the few
/// seconds an idle one does.
pub async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = test_kit::Deadline::generous();
    while !deadline.passed() {
        if check() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    deadline.fail(what);
}

/// What a rig's daemon has done so far, as one number that only grows: the cycles every dataset
/// of `hub` has begun and finished, plus the `requests` its fake server has served. A wait that
/// reads it ([`eventually_alive`], [`until_alive`]) counts each change as progress, so a loaded
/// machine that mirrors slowly is never cut short, and a daemon that stopped still is.
pub fn pulse(hub: &syncd::service::Hub, requests: usize) -> u64 {
    let cycles: u64 = hub
        .names()
        .iter()
        .filter_map(|name| hub.cycles(name))
        .map(|cycles| cycles.begun + cycles.ended)
        .sum();
    cycles + requests as u64
}

/// Polls `check` every 20 ms like [`eventually`], but the limit is idle time: the wait fails
/// only when `pulse` (see [`pulse`]) has not changed for [`test_kit::GENEROUS`], so the time a
/// scenario takes may grow with the machine's load as long as the daemon keeps working.
pub async fn eventually_alive(
    what: &str,
    mut pulse: impl FnMut() -> u64,
    mut check: impl FnMut() -> bool,
) {
    until_alive(what, &mut pulse, || std::future::ready(check())).await;
}

/// [`eventually_alive`] for a check that is itself asynchronous (a bus call).
pub async fn until_alive<F: std::future::Future<Output = bool>>(
    what: &str,
    mut pulse: impl FnMut() -> u64,
    mut check: impl FnMut() -> F,
) {
    let mut deadline = test_kit::Deadline::idle(test_kit::GENEROUS);
    let mut seen = pulse();
    loop {
        if check().await {
            return;
        }
        let now = pulse();
        if now != seen {
            seen = now;
            deadline.progress();
        }
        if deadline.passed() {
            deadline.fail(what);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// The next item of `stream`, waiting as [`until_alive`] does: while `pulse` changes the wait
/// goes on; `None` when the stream ended.
///
/// # Panics
/// When nothing came and `pulse` stayed the same for [`test_kit::GENEROUS`].
pub async fn next_alive<S: zbus::export::futures_core::Stream + Unpin>(
    what: &str,
    mut pulse: impl FnMut() -> u64,
    stream: &mut S,
) -> Option<S::Item> {
    let mut deadline = test_kit::Deadline::idle(test_kit::GENEROUS);
    let mut seen = pulse();
    loop {
        let next = std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx));
        if let Ok(item) = tokio::time::timeout(std::time::Duration::from_millis(20), next).await {
            return item;
        }
        let now = pulse();
        if now != seen {
            seen = now;
            deadline.progress();
        }
        if deadline.passed() {
            deadline.fail(what);
        }
    }
}
