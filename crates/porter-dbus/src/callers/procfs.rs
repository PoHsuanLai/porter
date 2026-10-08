//! Naming a process from what `/proc/<pid>` says of it: its cgroup, and its sandbox's own metadata
//! (`/proc/<pid>/root/.flatpak-info`). The metadata is read through the process's root, which is
//! ptrace-gated: a reader inside a Landlock domain is refused it, and then no process is a Flatpak
//! app (a scope's name proves nothing). The root is a parameter, so the tests read fixture trees.

use super::{Caller, CallerTable};
use porter_core::identity_of;
use porter_core::{AppId, CgroupPath, PeerFacts, PeerIdentity, SandboxFacts};
use std::path::Path;

/// The launcher namespace of app scopes: `app-[<launcher>-]<id>-<n>.scope`.
const APP_PREFIX: &str = "app-";

/// Where Flatpak puts its metadata, at the root of the sandbox's file system.
const FLATPAK_INFO: &str = ".flatpak-info";

/// The largest `.flatpak-info` read: the real one is a few hundred bytes.
const FLATPAK_INFO_MAX: u64 = 64 * 1024;

/// What a process's files say it is, before a service's main process is checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Found {
    /// An app: a Flatpak sandbox by its metadata, or a native app by its launcher's scope.
    App(Caller),
    /// A process in a service unit the table names. It is that caller only when it is the
    /// unit's main process: any process of the person's may move itself into the unit's cgroup.
    Service {
        /// The unit (`inferd.service`).
        unit: String,
        /// The row's caller.
        caller: Caller,
    },
    /// A process in a scope outside the `app-` namespace that the table names
    /// (`sill-shell.scope`). A scope has no main process to check.
    Scope(Caller),
}

/// What process `pid` under `proc_root` is, or `None` for a process nothing names.
///
/// - A sandbox's metadata (`<pid>/root/.flatpak-info`) names a Flatpak app, whatever its cgroup.
///   A metadata file that is there and does not read names nobody.
/// - `<name>.service`: the unit the table names, as that row's caller, to be checked.
/// - `<name>.scope` that is not an app scope (`sill-shell.scope`, started by
///   `systemd-run --scope --unit=sill-shell`): the same, matched exactly, never by prefix.
/// - `app-[<launcher>-]<id>-<n>.scope`: the native app `<id>`, with the role of its table row.
///   Flatpak's own `app-flatpak-…` scopes name nobody: a Flatpak app is known by its metadata.
/// - Anything else (a terminal's child, a session scope, an unlisted unit, no cgroup): nobody.
pub(super) fn found_of_pid(proc_root: &Path, pid: u32, table: &CallerTable) -> Option<Found> {
    let dir = proc_root.join(pid.to_string());
    let text = std::fs::read_to_string(dir.join("cgroup")).ok()?;
    let cgroup = CgroupPath::from_proc_cgroup(&text).ok()?;
    let sandbox = match read_capped(&dir.join("root").join(FLATPAK_INFO)) {
        Some(info) => flatpak_facts(&info)?,
        None => SandboxFacts::Native,
    };
    let leaf = cgroup.as_str().rsplit('/').next().unwrap_or_default();
    let native = sandbox == SandboxFacts::Native;
    if native && leaf.ends_with(".service") {
        let caller = table.resolve_unit(leaf)?;
        return Some(Found::Service {
            unit: leaf.to_owned(),
            caller,
        });
    }
    if native && leaf.ends_with(".scope") && !leaf.starts_with(APP_PREFIX) {
        return table.resolve_unit(leaf).map(Found::Scope);
    }
    let facts = PeerFacts {
        pid,
        cgroup,
        sandbox,
    };
    let app: AppId = match identity_of(&facts) {
        PeerIdentity::Proven(app) => app,
        PeerIdentity::Unproven => return None,
    };
    Some(Found::App(Caller {
        role: table.role_of(&app.name),
        app,
    }))
}

/// A small file's text; `None` when it is not there or cannot be read (a reader refused the
/// process's root reads it as a native process, which its cgroup then names).
fn read_capped(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(FLATPAK_INFO_MAX)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// The sandbox facts a `.flatpak-info` key file states: `[Application] name` and
/// `[Instance] instance-id`. `None` when it has no application name: a metadata file that is
/// there and names nothing is not a native process either.
fn flatpak_facts(info: &str) -> Option<SandboxFacts> {
    let mut section = "";
    let (mut app, mut instance) = (None, None);
    for line in info.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (section, key.trim()) {
            ("Application", "name") => app = Some(value.trim().to_owned()),
            ("Instance", "instance-id") => instance = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    Some(SandboxFacts::Flatpak {
        app: app.filter(|a| !a.is_empty())?,
        instance: instance.unwrap_or_default(),
    })
}

/// The main process of the unit `unit` as a fixture tree states it: the decimal pid in
/// `<proc_root>/units/<unit>`, the stand-in for the systemd manager's `MainPID` in a test. The
/// real `/proc` has no such file, so read there it is `None`.
pub(super) fn fixture_main_pid(proc_root: &Path, unit: &str) -> Option<u32> {
    if unit.contains('/') {
        return None;
    }
    std::fs::read_to_string(proc_root.join("units").join(unit))
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests;
