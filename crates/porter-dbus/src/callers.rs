//! Who is calling: a bus connection's unique name to a [`Caller`] (an app and a role), for both
//! daemons (porter PLAN §2.1). The process behind the connection is found through the bus
//! (pid), `/proc/<pid>/cgroup` and the Flatpak info, and named by `identity_of`; the role comes
//! from a table of executables (`/etc/porter/callers.toml`, a user file wins). An unidentified
//! sender is refused by the daemon (`AccessDenied`), never given a role.

use crate::BusConnection;
use zbus::fdo::DBusProxy;
use zbus::names::BusName;

mod procfs;
mod table_file;

pub use table_file::TableFileError;

use porter_core::{AppId, AppName, Isolation};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::{Path, PathBuf};

/// What a caller may ask for, beyond what its consent allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallerRole {
    /// Any identified app.
    App,
    /// detent: may use `SettingsModule1` (remove, toggle, revoke, client ids).
    Settings,
    /// sill, or a standalone sheet host: may serve `AccountsSheet1`.
    SheetHost,
    /// inferd and syncd: may use `Accounts1.Peer`.
    PorterDaemon,
    /// intentd, companiond, readerd, cuad, quire-do, actions-mcp: refused `Choose`,
    /// `AddAccount`, `Reauthenticate`, `IssueToken` and `OpenAuthenticated`; they keep inferd.
    Agent,
    /// cuad, for inferd: the only role that may open a computer-use session.
    Cua,
}

/// Who a connection is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The identity grants and audit entries bind to.
    pub app: AppId,
    /// What it may ask for.
    pub role: CallerRole,
}

/// One executable and who it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerRow {
    /// The program's path, as `/proc/<pid>/exe` reads it.
    pub exe: PathBuf,
    /// The app it is.
    pub app: AppName,
    /// Its role.
    pub role: CallerRole,
}

/// The table of executables: `[[caller]]` rows in `callers.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CallerTable {
    /// The rows.
    #[serde(rename = "caller", default)]
    pub callers: Vec<CallerRow>,
}

impl CallerTable {
    /// The system's table with the user's laid over it: a user row for an executable the system
    /// names replaces it.
    pub fn layered(system: CallerTable, user: CallerTable) -> Self {
        let mut callers: Vec<CallerRow> = system
            .callers
            .into_iter()
            .filter(|row| !user.callers.iter().any(|mine| mine.exe == row.exe))
            .collect();
        callers.extend(user.callers);
        Self { callers }
    }

    /// The role of the app `name`: its last row's (the user's rows come last), `App` for an app
    /// the table does not name.
    pub fn role_of(&self, name: &AppName) -> CallerRole {
        self.callers
            .iter()
            .rev()
            .find(|row| &row.app == name)
            .map_or(CallerRole::App, |row| row.role)
    }

    /// The caller whose executable is `exe`, as an unsandboxed native process. A sandboxed app
    /// is named by its Flatpak info instead, and takes its role from the row of its app.
    pub fn resolve(&self, exe: &Path) -> Option<Caller> {
        self.callers
            .iter()
            .find(|row| row.exe == exe)
            .map(|row| Caller {
                app: AppId {
                    name: row.app.clone(),
                    isolation: Isolation::Unsandboxed,
                },
                role: row.role,
            })
    }
}

/// Says who a bus connection is.
pub trait Callers: Send + Sync + 'static {
    /// The caller behind the connection `sender` (its unique name), or `None` for nobody known.
    fn caller_of(&self, sender: &str) -> impl Future<Output = Option<Caller>> + Send;
}

impl<T: Callers> Callers for std::sync::Arc<T> {
    async fn caller_of(&self, sender: &str) -> Option<Caller> {
        (**self).caller_of(sender).await
    }
}

/// Callers by process: the bus names the connection's pid, the cgroup and Flatpak info name the
/// app, the table names the role. The `/proc` root is the caller's to give, so a test reads a
/// fixture tree.
#[derive(Debug)]
pub struct ProcCallers {
    connection: BusConnection,
    table: CallerTable,
    proc_root: PathBuf,
}

impl ProcCallers {
    /// Resolves senders on `connection` through `table`, reading the system's `/proc`.
    pub fn new(connection: BusConnection, table: CallerTable) -> Self {
        Self::with_proc_root(connection, table, PathBuf::from("/proc"))
    }

    /// As [`ProcCallers::new`], reading `proc_root` instead of `/proc`.
    pub fn with_proc_root(
        connection: BusConnection,
        table: CallerTable,
        proc_root: PathBuf,
    ) -> Self {
        Self {
            connection,
            table,
            proc_root,
        }
    }

    /// The caller that is process `pid` of the `/proc` tree at `proc_root`, as [`ProcCallers`]
    /// reads it once the bus has named the pid.
    pub fn caller_of_pid(proc_root: &Path, pid: u32, table: &CallerTable) -> Option<Caller> {
        procfs::caller_of_pid(proc_root, pid, table)
    }

    /// The pid of the process behind `sender`, as the bus knows it.
    async fn pid_of(&self, sender: &str) -> Option<u32> {
        let name = BusName::try_from(sender).ok()?;
        let bus = DBusProxy::new(&self.connection).await.ok()?;
        bus.get_connection_credentials(name)
            .await
            .ok()?
            .process_id()
    }
}

impl Callers for ProcCallers {
    async fn caller_of(&self, sender: &str) -> Option<Caller> {
        let pid = self.pid_of(sender).await?;
        Self::caller_of_pid(&self.proc_root, pid, &self.table)
    }
}

#[cfg(test)]
mod tests;
