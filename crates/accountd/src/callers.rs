//! Who is calling: a bus sender's unique name to a caller, through the one table both daemons
//! share (`porter_dbus::Callers`, with `ProcCallers` for the real bus). [`TableCallers`] is the
//! table-backed one for tests and for hosts that know their clients.

use porter_core::AppId;
pub use porter_dbus::Callers;
use porter_dbus::{Caller, CallerRole};
use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

/// Callers from a map of unique names, for tests and for hosts that know their clients.
#[derive(Debug, Default)]
pub struct TableCallers(Mutex<BTreeMap<String, Caller>>);

impl TableCallers {
    /// Nobody is known.
    pub fn new() -> Self {
        Self::default()
    }

    /// Says that the connection `unique_name` is `app`, in the `App` role.
    pub fn introduce(&self, unique_name: &str, app: AppId) {
        self.introduce_as(
            unique_name,
            Caller {
                app,
                role: CallerRole::App,
            },
        );
    }

    /// Says that the connection `unique_name` is `caller`.
    pub fn introduce_as(&self, unique_name: &str, caller: Caller) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(unique_name.to_owned(), caller);
    }
}

impl Callers for TableCallers {
    async fn caller_of(&self, sender: &str) -> Option<Caller> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(sender)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{AppName, Isolation};

    fn app(name: &str) -> AppId {
        AppId {
            name: AppName::parse(name).expect("name"),
            isolation: Isolation::Flatpak,
        }
    }

    #[tokio::test]
    async fn introduced_connections_are_known_with_their_role_and_others_are_not() {
        let callers = std::sync::Arc::new(TableCallers::new());
        callers.introduce(":1.1", app("org.example.A"));
        callers.introduce_as(
            ":1.2",
            Caller {
                app: app("org.quire.Settings"),
                role: CallerRole::Settings,
            },
        );
        let role = |c: Option<Caller>| c.map(|c| c.role);
        assert_eq!(role(callers.caller_of(":1.1").await), Some(CallerRole::App));
        assert_eq!(
            role(callers.caller_of(":1.2").await),
            Some(CallerRole::Settings)
        );
        assert_eq!(callers.caller_of(":1.3").await, None);
    }
}
