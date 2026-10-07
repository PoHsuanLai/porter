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

/// Polls `check` every 20 ms for up to a minute: a passing check returns at once, and a loaded
/// machine (a load average over a hundred) needs far more than the few seconds an idle one does.
pub async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..3000 {
        if check() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("never happened: {what}");
}
