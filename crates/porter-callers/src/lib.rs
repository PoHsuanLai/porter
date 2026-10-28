//! Who is calling: a bus connection's unique name to a [`Caller`] (an app and a role), for the
//! daemons (porter PLAN §2.1). The process behind the connection is held and judged by
//! [peephole](https://github.com/PoHsuanLai/peephole) (`BusPeers`): the bus names the process (a
//! pidfd), `/proc/<pid>` and the systemd manager state what it is, and the judgement is one of
//! four standings, which [`ProcCallers`] turns into a [`Caller`]:
//!
//! - a Flatpak app, by its sandbox's own metadata cross-checked against its cgroup's scope (never
//!   by a scope's name alone), is `Proven(Flatpak)`; a snap, by the kernel's label, is
//!   `Proven(Snap)`;
//! - a native app that a launcher started in `app-<id>-<n>.scope`, from a root-owned program, is
//!   `Proven(Unsandboxed)` (advisory: any process of the person's may start such a scope);
//! - a service the table names by unit and program is that row's app and role, as far as the
//!   proof reaches: a fixed role ([`CallerRole::is_fixed`]) needs peephole's `Unit` or `Full`
//!   proof (the unit manager says the process is the unit's main process, from a fragment and
//!   drop-ins root owns, not transient, starting the row's program, which is root-only all the
//!   way up); with less, the caller is a plain app;
//! - anything else (a program in no row and no app scope, an image a person can write, a process
//!   that left) is nobody, and the daemon refuses it (`AccessDenied`), never giving it a role.
//!
//! The role comes from the table (`callers.toml`: the user's file wins, except for the fixed
//! roles; the daemons read the files), never from a scope's name: a row that names a unit gives its
//! role to that unit alone. A unit row names its programs with `exe`; without one a daemon unit
//! (`<name>.service`) is taken to be installed as [`installed`]`(Install::Libexec, name)`, and
//! any other unit has no program and so never reaches a fixed role. See [`CallerRow::images`].
//!
//! The one way to ask "who is this sender" is [`Callers::caller_of`], answered by [`ProcCallers`]:
//! a daemon builds one from its bus connection and the table, and asks it for every method call.
//! A daemon does not decide that a name it can see on the bus, or the well-known name it owns,
//! is a role.
//!
//! ```ignore
//! let callers = ProcCallers::new(connection.clone(), load_callers(&system, &user)?);
//! // In a method, with `Call { header, .. }: Call<'_>`:
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
//! Which files and which `/proc` a daemon reads its callers from is here too: the system's table
//! under `/etc/porter` ([`CALLERS`], [`INFERD_CALLERS`], found by [`SystemCallers`]) and the
//! `/proc` tree ([`ProcRoot`]). Both can be moved by a variable in a test build only, never in a
//! shipped one ([`Override`]), because they decide who a caller is. The library reads no
//! environment variable: the daemon passes in how it looks one up.
//!
//! ```
//! use std::ffi::OsString;
//! use porter_callers::{choose_proc_root, proc_notice, proc_path};
//!
//! // The daemon's own build decides whether it honours the variable; here, a test build.
//! let lookup = |name: &str| (name == "MYD_PROC_ROOT").then(|| OsString::from("/fixture/proc"));
//! let root = choose_proc_root(true, "MYD_PROC_ROOT", lookup);
//! assert_eq!(proc_path(&root), std::path::Path::new("/fixture/proc"));
//!
//! // A shipped build ignores the same variable.
//! let root = choose_proc_root(false, "MYD_PROC_ROOT", lookup);
//! assert_eq!(proc_path(&root), std::path::Path::new("/proc"));
//! assert!(proc_notice(&root, "myd", "MYD_PROC_ROOT").is_some());
//! ```
//!
//! What this cannot prove: every process of the person's runs as the same user. A same-user
//! process that edits the person's own systemd configuration or environment and waits for the
//! next start of a trusted service rides on that service's role (peephole's README states the
//! same residual); the shipped units sit in the system's folders so that `Full` fails for a unit
//! kept in a home directory. A scope row never carries a fixed role: a scope has no main process,
//! so the unit manager can only vouch for its program (`Exe`).

#[cfg(feature = "file")]
mod file;
mod images;
mod proc;
mod proc_root;
mod standing;
mod system;
mod translate;

#[cfg(feature = "test-proc-root")]
mod fixture;

#[cfg(feature = "file")]
#[doc = "The caller table as files, from feature `file`."]
pub use file::{CallerFileError, CallerTomlError, load_callers, table_from_file, table_from_toml};

pub use images::{Install, installed};
pub use proc::ProcCallers;
pub use proc_root::{Override, ProcRoot, choose_proc_root, proc_notice, proc_path};
pub use system::{
    CALLERS, INFERD_CALLERS, SYSTEM_CALLERS, SYSTEM_CALLERS_VAR, SystemCallers, SystemFile,
    TABLE_DIRS, held_by, held_under, root_held,
};

/// The systemd manager's bus name.
pub const SYSTEMD: &str = "org.freedesktop.systemd1";
/// The systemd manager's object.
pub const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
/// The interface of the systemd manager's object.
pub const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";

use porter_core::{AppId, AppName, Isolation};
use serde::{Deserialize, Serialize};
use std::future::Future;

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
    /// The programs the unit runs (absolute paths, one `*` allowed in a segment: peephole's
    /// patterns). A fixed role needs the unit manager to say the unit starts one of them, so the
    /// system's table writes them for the units that are not installed as a daemon. Without any,
    /// see [`CallerRow::images`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exe: Vec<String>,
    /// Its name as a person reads it (`name = "Sync"`), where accountd's Settings lists what the
    /// app may use. Without one accountd looks for the app's desktop entry.
    #[serde(default)]
    pub name: Option<AppTitle>,
}

impl CallerRow {
    /// The programs the row's unit may run: its `exe`, else, for a daemon unit
    /// (`<name>.service`, the name lower case, digits and dashes), where the desktop installs
    /// daemons ([`installed`] with [`Install::Libexec`]). A row without a unit, or a unit that is
    /// not named like a daemon and lists no `exe`, has none: it can be an app, never a service.
    pub fn images(&self) -> Vec<String> {
        if !self.exe.is_empty() {
            return self.exe.clone();
        }
        self.unit
            .as_deref()
            .and_then(images::daemon_name)
            .map(|name| installed(Install::Libexec, name))
            .unwrap_or_default()
    }
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

#[cfg(test)]
mod tests;
