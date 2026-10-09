//! Who is calling: a bus connection's unique name to a [`Caller`].
//!
//! The caller is derived by the transport, never sent: the bus says which process owns a
//! connection, and `porter_dbus::ProcCallers` (shared with accountd) names it. inferd's own
//! `[callers]` table of `inferd.toml` is rows of that shared table, keyed by systemd unit (never
//! by executable path: the cgroup is all `ProcCallers` reads): cuad's units with the `Cua` role,
//! each app's with `App`. This is advisory for unsandboxed processes (porter R12): a
//! process that can start an allowed unit or app scope can be that caller. cuad is a fixed unit and
//! the only caller that may open a computer-use session; an app is whichever unit the table
//! names for it, or any identified app scope.

use porter_core::{AppId, AppName};
use porter_dbus::{BusConnection, CallerRole, CallerRow, ProcCallers};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

/// What a caller may ask for, beyond what its data class and consent allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// An app or a daemon that asks for language, embedding and speech models.
    App,
    /// cuad: the one caller that may open a computer-use session.
    Cua,
    /// detent, the Settings app: the one caller of `org.quire.SettingsModule1`.
    Settings,
    /// The agent launcher (docket-acp): the one caller of `org.quire.Inference1.Agents`, and of
    /// nothing else here.
    AgentLauncher,
    /// An app that may choose where the assistant runs (`places` in the options of `Open`,
    /// `Prepare` and `Availability`): docket's companion, reader and intents daemons. It asks
    /// for models as an `App` does.
    Placer,
    /// The shell: it may list the places (`Inference1.Places`) and ask for nothing else.
    Shell,
}

/// The app docket's companion daemon is: with Settings and the shell, the callers of `Places`.
const COMPANION_APP: &str = "org.quire.Companion";

/// Why a caller table's text was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TableError {
    /// Not valid TOML of the table's shape.
    #[error("{0}")]
    Toml(#[from] toml::de::Error),
    /// A `settings` entry that is neither a `.service` unit nor an app name.
    #[error("settings: `{0}` is neither a .service unit nor an app name")]
    BadSettingsEntry(String),
    /// A `shell` entry that is neither a `.scope` nor a `.service` unit.
    #[error("shell: `{0}` is neither a .scope nor a .service unit")]
    BadShellEntry(String),
    /// A `places` entry that is not a unit of an app in `[callers.apps]`.
    #[error("places: `{0}` is not a unit listed under [callers.apps]")]
    PlacesNotAnApp(String),
}

/// Who a connection is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The identity grants and audit entries bind to.
    pub app: AppId,
    /// What it may ask for.
    pub role: Role,
}

impl Caller {
    /// Whether it may list the places: Settings, the shell and the companion.
    pub fn may_list_places(&self) -> bool {
        match self.role {
            Role::Settings | Role::Shell => true,
            Role::Placer => self.app.name.as_str() == COMPANION_APP,
            Role::App | Role::Cua | Role::AgentLauncher => false,
        }
    }

    /// Whether it may choose where the assistant runs.
    pub fn may_choose_places(&self) -> bool {
        self.role == Role::Placer
    }

    /// Whether it may list the computers that ask to use this one and answer one that is
    /// asking: Settings and the shell.
    pub fn may_answer_guests(&self) -> bool {
        matches!(self.role, Role::Settings | Role::Shell)
    }
}

/// Which systemd unit is which caller.
///
/// ```toml
/// cua = ["cuad.service"]
/// settings = ["org.quire.Settings.service"]
/// [apps]
/// "org.quire.Memory" = ["memoryd.service"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CallerTable {
    #[serde(default)]
    cua: BTreeSet<String>,
    /// The units of the agent launcher.
    #[serde(default)]
    agent_launcher: BTreeSet<String>,
    #[serde(default)]
    apps: BTreeMap<AppName, BTreeSet<String>>,
    /// The Settings app's units: the only callers of the settings module, each only as its unit's
    /// main process (as accountd's `callers.toml` row for Settings). An entry that is an app name
    /// (the form this table had, `"org.quire.Settings"`) stands for that app's unit,
    /// `<app>.service`: never for any process in an app scope of that name, which any program can
    /// start.
    #[serde(default)]
    settings: BTreeSet<String>,
    /// The shell's units (`sill-shell.scope`): the shell may list the places and ask for
    /// nothing else. A scope has no main process, so while the shell is not running another
    /// program of the person's could start a scope of that name (as in accountd's table).
    #[serde(default)]
    shell: BTreeSet<String>,
    /// The units, each already an app under `apps`, that may choose where the assistant runs
    /// (`places` in the options of `Open`, `Prepare` and `Availability`).
    #[serde(default)]
    places: BTreeSet<String>,
}

/// The unit and the app a `settings` entry stands for: a `.service` unit is the Settings app's;
/// an app name is that app's `<app>.service`. Anything else is no entry.
fn settings_unit(entry: &str) -> Option<(String, AppName)> {
    match entry.strip_suffix(".service") {
        Some(name) if !name.is_empty() => {
            Some((entry.to_owned(), AppName::parse(SETTINGS_APP).ok()?))
        }
        Some(_) => None,
        None => AppName::parse(entry)
            .ok()
            .map(|app| (format!("{entry}.service"), app)),
    }
}

impl CallerTable {
    /// A table naming cuad's units and each app's.
    pub fn new(cua: BTreeSet<String>, apps: BTreeMap<AppName, BTreeSet<String>>) -> Self {
        Self {
            cua,
            agent_launcher: BTreeSet::new(),
            apps,
            settings: BTreeSet::new(),
            shell: BTreeSet::new(),
            places: BTreeSet::new(),
        }
    }

    /// The same table, naming `units` as the shell's.
    pub fn with_shell(self, units: BTreeSet<String>) -> Self {
        Self {
            shell: units,
            ..self
        }
    }

    /// The same table, naming `units` (each also an app's unit under `apps`) as callers that
    /// may choose where the assistant runs.
    pub fn with_places(self, units: BTreeSet<String>) -> Self {
        Self {
            places: units,
            ..self
        }
    }

    /// The same table, naming `units` as the agent launcher's.
    pub fn with_agent_launcher(self, units: BTreeSet<String>) -> Self {
        Self {
            agent_launcher: units,
            ..self
        }
    }

    /// The same table, naming `units` (or app names, each standing for `<app>.service`) as the
    /// Settings app's, the only callers of the settings module.
    pub fn with_settings(self, units: BTreeSet<String>) -> Self {
        Self {
            settings: units,
            ..self
        }
    }

    /// The table in TOML text. A `settings` entry that is neither a `.service` unit nor an app
    /// name is refused.
    pub fn from_toml_text(text: &str) -> Result<Self, TableError> {
        let table: Self = toml::from_str(text)?;
        if let Some(bad) = table.settings.iter().find(|e| settings_unit(e).is_none()) {
            return Err(TableError::BadSettingsEntry(bad.clone()));
        }
        let unit_like = |e: &&String| e.ends_with(".scope") || e.ends_with(".service");
        if let Some(bad) = table.shell.iter().find(|e| !unit_like(e)) {
            return Err(TableError::BadShellEntry(bad.clone()));
        }
        let listed = |unit: &&String| table.apps.values().any(|units| units.contains(*unit));
        match table.places.iter().find(|unit| !listed(unit)) {
            Some(bad) => Err(TableError::PlacesNotAnApp(bad.clone())),
            None => Ok(table),
        }
    }

    /// The rows of the shared table this table is: cuad's first, so an app entry cannot claim its
    /// unit (the shared table answers with the first row that names one).
    pub fn rows(&self) -> porter_dbus::CallerTable {
        let cua_app = AppName::parse(CUA_APP).ok();
        let cua = cua_app.into_iter().flat_map(|app| {
            self.cua.iter().map(move |unit| CallerRow {
                unit: Some(unit.clone()),
                app: app.clone(),
                role: CallerRole::Cua,
                name: None,
            })
        });
        let launcher_app = AppName::parse(LAUNCHER_APP).ok();
        let launcher = launcher_app.into_iter().flat_map(|app| {
            self.agent_launcher.iter().map(move |unit| CallerRow {
                unit: Some(unit.clone()),
                app: app.clone(),
                role: CallerRole::AgentLauncher,
                name: None,
            })
        });
        // Settings by its unit, whose main process alone has the role (porter_dbus::MainPids):
        // before the apps, so an app entry cannot claim the unit.
        let settings = self
            .settings
            .iter()
            .filter_map(|entry| settings_unit(entry))
            .map(|(unit, app)| CallerRow {
                unit: Some(unit),
                app,
                role: CallerRole::Settings,
                name: None,
            });
        // The shell by its scope; the shared role is the one accountd gives it.
        let shell_app = AppName::parse(SHELL_APP).ok();
        let shell = shell_app.into_iter().flat_map(|app| {
            self.shell.iter().map(move |unit| CallerRow {
                unit: Some(unit.clone()),
                app: app.clone(),
                role: CallerRole::SheetHost,
                name: None,
            })
        });
        // An app whose unit is under `places` is a `Placer`, carried by the shared role `Agent`
        // (companiond, readerd and intentd are the ones accountd calls so).
        let apps = self.apps.iter().flat_map(|(app, units)| {
            units.iter().map(move |unit| CallerRow {
                unit: Some(unit.clone()),
                app: app.clone(),
                role: if self.places.contains(unit) {
                    CallerRole::Agent
                } else {
                    CallerRole::App
                },
                name: None,
            })
        });
        porter_dbus::CallerTable {
            callers: cua
                .chain(launcher)
                .chain(settings)
                .chain(shell)
                .chain(apps)
                .collect(),
        }
    }

    /// The caller that is the unit `unit`, through [`CallerTable::rows`].
    pub fn resolve(&self, unit: &str) -> Option<Caller> {
        self.rows().resolve_unit(unit).map(Caller::from_shared)
    }
}

/// The app cuad is.
const CUA_APP: &str = "org.quire.Cua";

/// The app the agent launcher is.
const LAUNCHER_APP: &str = "org.quire.AgentLauncher";

/// The Settings app, which a `settings` entry that is a unit runs.
const SETTINGS_APP: &str = "org.quire.Settings";

/// The shell, which a `shell` entry runs.
const SHELL_APP: &str = "org.quire.Shell";

impl Caller {
    /// A shared caller as inferd knows roles: `Cua` and `Settings`, and `App` for every other.
    pub fn from_shared(caller: porter_dbus::Caller) -> Self {
        Self {
            app: caller.app,
            role: match caller.role {
                CallerRole::Cua => Role::Cua,
                CallerRole::Settings => Role::Settings,
                CallerRole::AgentLauncher => Role::AgentLauncher,
                CallerRole::Agent => Role::Placer,
                CallerRole::SheetHost => Role::Shell,
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
        Self::with_root(connection, table, &ProcRoot::System)
    }

    /// As [`ProcPeers::new`], reading the process tree `root` names.
    pub fn with_root(connection: BusConnection, table: CallerTable, root: &ProcRoot) -> Self {
        let rows = table.rows();
        let callers = match root {
            ProcRoot::Fake(dir) => ProcCallers::with_proc_root(connection, rows, dir.clone()),
            ProcRoot::System | ProcRoot::Ignored(_) => ProcCallers::new(connection, rows),
        };
        Self { callers }
    }
}

/// Whether this build honours `INFERD_PROC_ROOT`: only with the `test-proc-root` feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcGate {
    /// The variable names the process tree.
    Honour,
    /// The variable is ignored.
    Ignore,
}

impl ProcGate {
    /// What this build does.
    pub const BUILT: Self = if cfg!(feature = "test-proc-root") {
        Self::Honour
    } else {
        Self::Ignore
    };
}

/// Where the caller lookup reads processes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcRoot {
    /// The system's `/proc`.
    System,
    /// A fixture tree: `<dir>/<pid>/cgroup`.
    Fake(PathBuf),
    /// The variable was set but this build ignores it.
    Ignored(PathBuf),
}

impl ProcRoot {
    /// The root for `INFERD_PROC_ROOT`'s value `var` under `gate`.
    pub fn select(gate: ProcGate, var: Option<&str>) -> Self {
        match (gate, var.filter(|dir| !dir.is_empty())) {
            (_, None) => Self::System,
            (ProcGate::Honour, Some(dir)) => Self::Fake(PathBuf::from(dir)),
            (ProcGate::Ignore, Some(dir)) => Self::Ignored(PathBuf::from(dir)),
        }
    }

    /// The line for standard error at start, when the variable was set.
    pub fn notice(&self) -> Option<String> {
        match self {
            Self::System => None,
            Self::Fake(dir) => Some(format!(
                "inferd: test proc root {}: callers are read from it, not /proc",
                dir.display()
            )),
            Self::Ignored(dir) => Some(format!(
                "inferd: INFERD_PROC_ROOT={} ignored: not a test-proc-root build",
                dir.display()
            )),
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
