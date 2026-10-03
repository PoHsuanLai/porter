//! Who is calling: a bus connection's unique name to a [`Caller`].
//!
//! The caller is derived by the transport, never sent: the bus says which process owns a
//! connection, and that process's executable decides the caller through the caller table
//! (`[callers]` of `inferd.toml`). This is advisory for unsandboxed processes (porter R12): a
//! process that can run an allowed executable can be that caller. cuad is a fixed executable and
//! the only caller that may open a computer-use session; an app is whichever executable the
//! table names for it.

use porter_core::{AppId, AppName, Isolation};
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

    /// The caller whose executable is `exe`. cuad is checked first, so an app entry cannot
    /// claim its executable.
    pub fn resolve(&self, exe: &Path) -> Option<Caller> {
        let unsandboxed = |name: &str| {
            Some(AppId {
                name: AppName::parse(name).ok()?,
                isolation: Isolation::Unsandboxed,
            })
        };
        if self.cua.contains(exe) {
            return Some(Caller {
                app: unsandboxed("org.quire.Cua")?,
                role: Role::Cua,
            });
        }
        self.apps
            .iter()
            .find(|(_, exes)| exes.contains(exe))
            .map(|(name, _)| Caller {
                app: AppId {
                    name: name.clone(),
                    isolation: Isolation::Unsandboxed,
                },
                role: Role::App,
            })
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

/// Peers by process: the bus names the connection's pid, `/proc/<pid>/exe` names the program,
/// the [`CallerTable`] names the caller.
#[derive(Debug)]
pub struct ProcPeers {
    connection: zbus::Connection,
    table: CallerTable,
}

impl ProcPeers {
    /// Resolves senders on `connection` through `table`.
    pub fn new(connection: zbus::Connection, table: CallerTable) -> Self {
        Self { connection, table }
    }
}

/// The program a process runs. A replaced binary reads back with `" (deleted)"` appended, which
/// is not the program that was allowed.
fn exe_of(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

impl Peers for ProcPeers {
    async fn caller_of(&self, sender: &str) -> Option<Caller> {
        let name = zbus::names::BusName::try_from(sender).ok()?;
        let bus = zbus::fdo::DBusProxy::new(&self.connection).await.ok()?;
        let pid = bus.get_connection_unix_process_id(name).await.ok()?;
        exe_of(pid).and_then(|exe| self.table.resolve(&exe))
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
