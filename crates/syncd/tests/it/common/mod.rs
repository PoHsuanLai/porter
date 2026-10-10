//! The rig every bus test of syncd uses: a private bus, callers named by unique name, clients.
//! Nothing here reaches the real session bus, `~/.config` or the network.
#![allow(dead_code)]

pub mod bus;

use bus::PrivateBus;
use porter_core::{AppId, AppName, Isolation};
use porter_dbus::{Caller, CallerRole, Callers};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

pub const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";
pub const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";
pub const NO_FITTING: &str = "org.quire.Accounts1.Error.NoFittingAccount";

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

/// Polls `check` every 20 ms for up to [`porter_fake::GENEROUS`] by the clock (not by counting
/// sleeps, which a loaded machine stretches several times over): a passing check returns at
/// once, and a loaded machine (a load average over a hundred) needs far more than the few
/// seconds an idle one does.
pub async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if check() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    deadline.fail(what);
}
