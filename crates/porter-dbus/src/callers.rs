//! Who is calling: a bus connection's unique name to a [`Caller`] (an app and a role), for both
//! daemons (porter PLAN §2.1). The process behind the connection is found through the bus
//! (pid), `/proc/<pid>/cgroup` and the Flatpak info, and named by `identity_of`; the role comes
//! from a table of executables (`/etc/porter/callers.toml`, a user file wins). An unidentified
//! sender is refused by the daemon (`AccessDenied`), never given a role.

use crate::BusConnection;
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
/// app, the table names the role.
#[derive(Debug)]
pub struct ProcCallers {
    connection: BusConnection,
    table: CallerTable,
}

impl ProcCallers {
    /// Resolves senders on `connection` through `table`.
    pub fn new(connection: BusConnection, table: CallerTable) -> Self {
        Self { connection, table }
    }
}

impl Callers for ProcCallers {
    async fn caller_of(&self, sender: &str) -> Option<Caller> {
        let _ = (&self.connection, &self.table, sender);
        todo!(
            "GetConnectionCredentials for the pid, read /proc/<pid>/cgroup and the Flatpak info, \
             `identity_of` for the app, the table for the role; None for a scope-less caller"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(exe: &str, app: &str, role: CallerRole) -> CallerRow {
        CallerRow {
            exe: PathBuf::from(exe),
            app: AppName::parse(app).expect("name"),
            role,
        }
    }

    fn table() -> CallerTable {
        CallerTable {
            callers: vec![
                row(
                    "/usr/libexec/quire/inferd",
                    "org.quire.Inference",
                    CallerRole::PorterDaemon,
                ),
                row(
                    "/usr/bin/detent",
                    "org.quire.Settings",
                    CallerRole::Settings,
                ),
                row("/usr/libexec/quire/cuad", "org.quire.Cua", CallerRole::Cua),
            ],
        }
    }

    #[test]
    fn an_executable_resolves_to_its_app_and_role() {
        let cases = [
            (
                "/usr/libexec/quire/inferd",
                Some(("org.quire.Inference", CallerRole::PorterDaemon)),
            ),
            (
                "/usr/bin/detent",
                Some(("org.quire.Settings", CallerRole::Settings)),
            ),
            ("/usr/bin/detent (deleted)", None),
            ("/usr/bin/bash", None),
        ];
        for (exe, want) in cases {
            let got = table()
                .resolve(Path::new(exe))
                .map(|c| (c.app.name.to_string(), c.role));
            assert_eq!(got, want.map(|(n, r)| (n.to_owned(), r)), "{exe}");
        }
        assert_eq!(
            table()
                .resolve(Path::new("/usr/bin/detent"))
                .map(|c| c.app.isolation),
            Some(Isolation::Unsandboxed)
        );
    }

    #[test]
    fn a_user_row_replaces_the_system_row_of_the_same_executable() {
        let user = CallerTable {
            callers: vec![row(
                "/usr/bin/detent",
                "org.example.MySettings",
                CallerRole::App,
            )],
        };
        let merged = CallerTable::layered(table(), user);
        let got = merged.resolve(Path::new("/usr/bin/detent")).expect("named");
        assert_eq!(
            (got.app.name.as_str(), got.role),
            ("org.example.MySettings", CallerRole::App)
        );
        assert_eq!(merged.callers.len(), 3);
    }

    #[test]
    fn a_table_round_trips_through_its_serde_form() {
        let json = serde_json::to_string(&table()).expect("json");
        assert_eq!(
            serde_json::from_str::<CallerTable>(&json).expect("table"),
            table()
        );
        assert!(json.contains(r#""role":"porter_daemon""#));
    }
}
