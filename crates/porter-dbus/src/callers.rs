//! Who is calling: a bus connection's unique name to a [`Caller`] (an app and a role), for both
//! daemons (porter PLAN §2.1). The process behind the connection is found through the bus (pid),
//! then `/proc/<pid>`: a Flatpak app by its sandbox's own metadata (`root/.flatpak-info`, never
//! by its scope's name), `app-<id>-<n>.scope` a native app (named by `identity_of`),
//! `<name>.service` a daemon's unit when the process is that unit's main process (the systemd
//! manager's `MainPID`), `<name>.scope` outside `app-` a scope the table names. The role comes
//! from a table keyed by app id and unit name (`callers.toml`: the user's file wins, except for
//! the fixed roles; the daemons read the files). An unidentified sender is refused by the daemon (`AccessDenied`), never given a
//! role.
//!
//! The one way to ask "who is this sender" is [`Callers::caller_of`], answered by [`ProcCallers`]:
//! a daemon builds one from its bus connection and the table, and asks it for every method call.
//! A daemon does not decide that a name it can see on the bus, or the well-known name it owns,
//! is a role.
//!
//! ```ignore
//! let callers = ProcCallers::new(connection.clone(), load_callers(&system, &user)?);
//! // In a method, with `#[zbus(header)] header: Header<'_>`:
//! let sender = header.sender().ok_or(fdo::Error::AccessDenied("no sender".into()))?;
//! let caller = callers.caller_of(sender.as_str()).await
//!     .ok_or(fdo::Error::AccessDenied("unknown caller".into()))?;
//! if caller.role != CallerRole::SheetHost { return Err(fdo::Error::AccessDenied("not the shell".into())); }
//! ```
//!
//! Some roles are fixed ([`CallerRole::is_fixed`]): only the system's table (`/etc`, the shipped
//! file) can give them, never the user's file. A user row may add a plain app or lower a role;
//! [`CallerTable::layered`] drops a user row that claims a fixed role.
//!
//! What this cannot prove: every process of the person's runs as the same user, so a service row
//! is as strong as the manager's word on its main process, and a scope row as strong as "the
//! scope was there first". That is why a scope never carries a fixed role: a row that gives one
//! to a scope gives it a plain app's. A Flatpak app's metadata is
//! read through its root, which a reader inside a Landlock domain is refused: there no caller is
//! a Flatpak app.

use crate::BusConnection;
#[cfg(feature = "callers-file")]
#[doc = "The caller table as files, from feature `callers-file`."]
pub use crate::callers_file::{
    CallerFileError, CallerTomlError, load_callers, table_from_file, table_from_toml,
};
use zbus::fdo::DBusProxy;
use zbus::names::BusName;

mod procfs;

use porter_core::{AppId, AppName, Isolation};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::{Path, PathBuf};

/// What a caller may ask for, beyond what its consent allows. It grows as the desktop gains
/// processes with a standing of their own, so a match outside porter needs a `_` arm (an
/// unknown role is best treated as a plain app).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CallerRole {
    /// Any identified app.
    App,
    /// detent: may use `SettingsModule1` (remove, toggle, revoke, client ids).
    Settings,
    /// sill, or a standalone sheet host: may serve `AccountsSheet1`.
    SheetHost,
    /// inferd and syncd, as the peers of one another and of the desktop's daemons. accountd no
    /// longer has an interface for them: it keeps identities, and nothing a daemon asks for on
    /// an app's behalf.
    PorterDaemon,
    /// intentd, companiond, readerd, cuad, quire-do, actions-mcp: refused inferd's `Keys`
    /// (`RequestKey`, `AddAccount`) and accountd's `AddAccount` and `Reauthenticate`; they keep
    /// inferd's own inference calls.
    Agent,
    /// cuad, for inferd: the only role that may open a computer-use session.
    Cua,
    /// The agent launcher (docket-acp), which runs external coding agents: may call inferd's
    /// `Launcher` interface (`SetAgentState`, `RegisterLauncher`, `BeginSession`, `EndSession`,
    /// `RequestAgentKey`, `ReportAgentLogin`, `ReportAgentLogout`, `IssueProcessCredential` and
    /// `RevokeProcessCredential`: an API key for a process it spawns, only for a program it
    /// registered), and nothing else. Read from inferd's caller table, not accountd's. Never
    /// granted by default: `dist/inferd.toml` has no row for it, so a machine lists the launcher
    /// by its own row.
    AgentLauncher,
    /// The terminal (temor): may read the person's computers from `org.quire.Tailnet1`
    /// (`Machines`) and nothing else of accountd. Given by the unit that is the terminal, as the
    /// systemd manager says its main process is, never by an app scope's name.
    Terminal,
    /// The compositor (casement): mints a desktop-wide Space id for each workspace and
    /// registers it with `org.quire.Spaces1` (`Register`); it may `List` the Spaces and nothing
    /// else of accountd. Given by the compositor's unit, never by an app scope's name.
    Compositor,
}

impl CallerRole {
    /// Whether the role stands for more than a plain app, so only the system's table may give
    /// it: `Settings`, `SheetHost`, `PorterDaemon`, `AgentLauncher`, `Cua`, `Terminal` and
    /// `Compositor`. `App` and `Agent` hold no standing beyond an app (`Agent` is refused more
    /// than `App` is), so a user's file may name them.
    pub const fn is_fixed(self) -> bool {
        !matches!(self, Self::App | Self::Agent)
    }
}

/// Who a connection is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The identity grants and audit entries bind to.
    pub app: AppId,
    /// What it may ask for.
    pub role: CallerRole,
}

/// What a person reads for an app: "Sync", not `org.quire.Sync`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AppTitle(pub String);

/// One app or unit and its role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerRow {
    /// The app: what an app scope or Flatpak scope names, or what the unit is known as.
    pub app: AppName,
    /// The systemd service unit (`inferd.service`) that is this app, for daemons that run in
    /// their own unit. Without one the row only gives the app its role.
    #[serde(default)]
    pub unit: Option<String>,
    /// Its role.
    pub role: CallerRole,
    /// Its name as a person reads it (`name = "Sync"`), where accountd's Settings lists what the
    /// app may use. Without one accountd looks for the app's desktop entry.
    #[serde(default)]
    pub name: Option<AppTitle>,
}

/// The table of apps and units: `[[caller]]` rows in `callers.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CallerTable {
    /// The rows.
    #[serde(rename = "caller", default)]
    pub callers: Vec<CallerRow>,
}

impl CallerTable {
    /// The system's table with the user's laid over it: a user row replaces the system's row
    /// for the same unit, or for the same app when neither names a unit. A user row may add a
    /// plain app or lower a role, never give a fixed role ([`CallerRole::is_fixed`]). Nor may it
    /// add a row for an app the system's table names (intentd, the companion, Settings…): such
    /// an app's identity is its system rows, which docket and memoryd answer by, so a user row
    /// for it is kept only where it replaces one of them. A row left out gets one warning on
    /// stderr.
    pub fn layered(system: CallerTable, user: CallerTable) -> Self {
        let same = |a: &CallerRow, b: &CallerRow| match (&a.unit, &b.unit) {
            (Some(x), Some(y)) => x == y,
            (None, None) => a.app == b.app,
            _ => false,
        };
        let owned = |row: &CallerRow| system.callers.iter().any(|mine| mine.app == row.app);
        let replaces_its_own = |row: &CallerRow| {
            system
                .callers
                .iter()
                .any(|mine| mine.app == row.app && same(mine, row))
        };
        let user = CallerTable {
            callers: user
                .callers
                .into_iter()
                .filter(|row| {
                    let why = if row.role.is_fixed() {
                        Some("only the system's table gives that role")
                    } else if owned(row) && !replaces_its_own(row) {
                        Some("the system's table owns that app")
                    } else {
                        None
                    };
                    if let Some(why) = why {
                        eprintln!(
                            "porter: callers: ignoring the user's row for {} ({:?}): {why}",
                            row.unit.as_deref().unwrap_or(row.app.as_str()),
                            row.role
                        );
                    }
                    why.is_none()
                })
                .collect(),
        };
        let mut callers: Vec<CallerRow> = system
            .callers
            .into_iter()
            .filter(|row| !user.callers.iter().any(|mine| same(mine, row)))
            .collect();
        callers.extend(user.callers);
        Self { callers }
    }

    /// The role of the app `name`, named by a scope: its last row without a unit (the user's rows
    /// come last), `App` for an app the table does not name. A row that names a unit gives its
    /// role only to that unit ([`CallerTable::resolve_unit`]): any process may start a scope
    /// called `app-<id>-<n>.scope`, so a unit's role reached by app name would be anyone's.
    pub fn role_of(&self, name: &AppName) -> CallerRole {
        self.callers
            .iter()
            .rev()
            .filter(|row| row.unit.is_none())
            .find(|row| &row.app == name)
            .map_or(CallerRole::App, |row| row.role)
    }

    /// The name a row gives the app `name`: its last row that has one (the user's rows come
    /// last), whether or not the row names a unit.
    pub fn title_of(&self, name: &AppName) -> Option<&AppTitle> {
        self.callers
            .iter()
            .rev()
            .filter(|row| &row.app == name)
            .find_map(|row| row.name.as_ref())
    }

    /// The caller that is the unit `unit` (a service, or a scope outside the `app-` namespace),
    /// an unsandboxed native process. The first row for a unit wins.
    pub fn resolve_unit(&self, unit: &str) -> Option<Caller> {
        self.callers
            .iter()
            .find(|row| row.unit.as_deref() == Some(unit))
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

/// Where a service unit's main process is learnt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainPids {
    /// The systemd manager on the connection's bus (`org.freedesktop.systemd1`, the person's
    /// user manager on the session bus): `GetUnit`, then the unit's `Service.MainPID`.
    Manager,
    /// A fixture tree: `<dir>/units/<unit>` holds the main process's pid (a test's stand-in for
    /// the manager).
    Fixture(PathBuf),
}

/// The systemd manager's bus name.
pub const SYSTEMD: &str = "org.freedesktop.systemd1";
/// The systemd manager's object.
pub const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
/// The interface of the systemd manager's object.
pub const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";

/// Callers by process: the bus names the connection's pid, `/proc/<pid>` names the app or unit,
/// the table names the role, and a service unit's role goes only to its main process. The
/// `/proc` root is the caller's to give, so a test reads a fixture tree.
#[derive(Debug)]
pub struct ProcCallers {
    connection: BusConnection,
    table: CallerTable,
    proc_root: PathBuf,
    main_pids: MainPids,
}

impl ProcCallers {
    /// Resolves senders on `connection` through `table`, reading the system's `/proc` and asking
    /// the systemd manager for a service's main process.
    pub fn new(connection: BusConnection, table: CallerTable) -> Self {
        Self {
            main_pids: MainPids::Manager,
            ..Self::with_proc_root(connection, table, PathBuf::from("/proc"))
        }
    }

    /// As [`ProcCallers::new`], reading the fixture tree `proc_root` instead of `/proc`, with a
    /// service's main process from the same tree ([`MainPids::Fixture`]).
    pub fn with_proc_root(
        connection: BusConnection,
        table: CallerTable,
        proc_root: PathBuf,
    ) -> Self {
        Self {
            connection,
            table,
            main_pids: MainPids::Fixture(proc_root.clone()),
            proc_root,
        }
    }

    /// The same, learning a service's main process from `main_pids`.
    pub fn with_main_pids(self, main_pids: MainPids) -> Self {
        Self { main_pids, ..self }
    }

    /// The caller that is process `pid` of the `/proc` tree at `proc_root`, as [`ProcCallers`]
    /// reads it once the bus has named the pid, with a service's main process from the same
    /// tree ([`MainPids::Fixture`]). Read on the real `/proc`, which states no main process, a
    /// service unit's row names nobody: [`ProcCallers`] asks the manager.
    pub fn caller_of_pid(proc_root: &Path, pid: u32, table: &CallerTable) -> Option<Caller> {
        match procfs::found_of_pid(proc_root, pid, table)? {
            procfs::Found::App(caller) | procfs::Found::Scope(caller) => Some(caller),
            procfs::Found::Service { unit, caller } => {
                (procfs::fixture_main_pid(proc_root, &unit) == Some(pid)).then_some(caller)
            }
        }
    }

    /// The main process of the service `unit`, as `main_pids` learns it.
    async fn main_pid(&self, unit: &str) -> Option<u32> {
        match &self.main_pids {
            MainPids::Fixture(dir) => procfs::fixture_main_pid(dir, unit),
            MainPids::Manager => self.manager_main_pid(unit).await,
        }
    }

    /// `MainPID` of the loaded unit `unit` from the systemd manager on the connection's bus;
    /// `None` when the manager is not there, the unit is not loaded or it has no main process.
    async fn manager_main_pid(&self, unit: &str) -> Option<u32> {
        let reply = self
            .connection
            .call_method(
                Some(SYSTEMD),
                SYSTEMD_PATH,
                Some(SYSTEMD_MANAGER),
                "GetUnit",
                &(unit,),
            )
            .await
            .ok()?;
        let path: zbus::zvariant::OwnedObjectPath = reply.body().deserialize().ok()?;
        let properties = zbus::fdo::PropertiesProxy::builder(&self.connection)
            .destination(SYSTEMD)
            .ok()?
            .path(path)
            .ok()?
            .build()
            .await
            .ok()?;
        let service =
            zbus::names::InterfaceName::try_from("org.freedesktop.systemd1.Service").ok()?;
        let value = properties.get(service, "MainPID").await.ok()?;
        u32::try_from(value).ok().filter(|pid| *pid != 0)
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
        match procfs::found_of_pid(&self.proc_root, pid, &self.table)? {
            procfs::Found::App(caller) | procfs::Found::Scope(caller) => Some(caller),
            procfs::Found::Service { unit, caller } => {
                (self.main_pid(&unit).await == Some(pid)).then_some(caller)
            }
        }
    }
}

#[cfg(test)]
mod tests;
