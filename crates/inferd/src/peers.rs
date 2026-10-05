//! Who is calling: a bus connection's unique name to a [`Caller`].
//!
//! The caller is derived by the transport, never sent: the bus says which process owns a
//! connection, and `porter_dbus::ProcCallers` (shared with accountd) names it. inferd's own
//! `[callers]` table of `inferd.toml` is rows of that shared table: cuad's executables with the
//! `Cua` role, each app's with `App`. This is advisory for unsandboxed processes (porter R12): a
//! process that can run an allowed executable can be that caller. cuad is a fixed executable and
//! the only caller that may open a computer-use session; an app is whichever executable the
//! table names for it.

use porter_core::{AppId, AppName};
use porter_dbus::{BusConnection, CallerRole, CallerRow, ProcCallers};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// What a caller may ask for, beyond what its data class and consent allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// An app or a daemon that asks for language, embedding and speech models.
    App,
    /// cuad: the one caller that may open a computer-use session.
    Cua,
}

/// Who a connection is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The identity grants and audit entries bind to.
    pub app: AppId,
    /// What it may ask for.
    pub role: Role,
}

/// Which executable is which caller.
///
/// ```toml
/// cua = ["/usr/libexec/quire/cuad"]
/// [apps]
/// "org.quire.Memory" = ["/usr/libexec/quire/memoryd"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CallerTable {
    #[serde(default)]
    cua: BTreeSet<PathBuf>,
    #[serde(default)]
    apps: BTreeMap<AppName, BTreeSet<PathBuf>>,
}

impl CallerTable {
    /// A table naming cuad's executables and each app's.
    pub fn new(cua: BTreeSet<PathBuf>, apps: BTreeMap<AppName, BTreeSet<PathBuf>>) -> Self {
        Self { cua, apps }
    }

    /// The table in TOML text.
    pub fn from_toml_text(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// The rows of the shared table this table is: cuad's first, so an app entry cannot claim its
    /// executable (the shared table answers with the first row that names one).
    pub fn rows(&self) -> porter_dbus::CallerTable {
        let cua_app = AppName::parse(CUA_APP).ok();
        let cua = cua_app.into_iter().flat_map(|app| {
            self.cua.iter().map(move |exe| CallerRow {
                exe: exe.clone(),
                app: app.clone(),
                role: CallerRole::Cua,
            })
        });
        let apps = self.apps.iter().flat_map(|(app, exes)| {
            exes.iter().map(move |exe| CallerRow {
                exe: exe.clone(),
                app: app.clone(),
                role: CallerRole::App,
            })
        });
        porter_dbus::CallerTable {
            callers: cua.chain(apps).collect(),
        }
    }

    /// The caller whose executable is `exe`, through [`CallerTable::rows`].
    pub fn resolve(&self, exe: &Path) -> Option<Caller> {
        self.rows().resolve(exe).map(Caller::from_shared)
    }
}

/// The app cuad is.
const CUA_APP: &str = "org.quire.Cua";

impl Caller {
    /// A shared caller as inferd knows roles: the `Cua` role, and `App` for every other.
    pub fn from_shared(caller: porter_dbus::Caller) -> Self {
        Self {
            app: caller.app,
            role: match caller.role {
                CallerRole::Cua => Role::Cua,
                _ => Role::App,
            },
        }
    }
}

/// Who a bus connection is.
pub trait Peers: Send + Sync + 'static {
    /// The caller behind the connection `sender` (its unique name), or `None` if it is nobody the
    /// table names.
    fn caller_of(&self, sender: &str) -> impl Future<Output = Option<Caller>> + Send;
}

impl<T: Peers> Peers for std::sync::Arc<T> {
    async fn caller_of(&self, sender: &str) -> Option<Caller> {
        (**self).caller_of(sender).await
    }
}

/// Peers by process: [`ProcCallers`] over the rows of the [`CallerTable`].
#[derive(Debug)]
pub struct ProcPeers {
    callers: ProcCallers,
}

impl ProcPeers {
    /// Resolves senders on `connection` through `table`.
    pub fn new(connection: BusConnection, table: CallerTable) -> Self {
        Self {
            callers: ProcCallers::new(connection, table.rows()),
        }
    }
}

impl Peers for ProcPeers {
    async fn caller_of(&self, sender: &str) -> Option<Caller> {
        porter_dbus::Callers::caller_of(&self.callers, sender)
            .await
            .map(Caller::from_shared)
    }
}

/// Peers from a map of unique names, for tests and for hosts that know their clients.
#[derive(Debug, Default)]
pub struct TablePeers(Mutex<BTreeMap<String, Caller>>);

impl TablePeers {
    /// Nobody is known.
    pub fn new() -> Self {
        Self::default()
    }

    /// Says that the connection `unique_name` is `caller`.
    pub fn introduce(&self, unique_name: &str, caller: Caller) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(unique_name.to_owned(), caller);
    }
}

impl Peers for TablePeers {
    async fn caller_of(&self, sender: &str) -> Option<Caller> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(sender)
            .cloned()
    }
}

#[cfg(test)]
mod tests;
