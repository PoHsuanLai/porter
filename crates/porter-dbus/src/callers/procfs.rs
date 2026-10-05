//! Reading a process's facts out of a `/proc` tree and naming its caller. The root is a
//! parameter, so the tests read fixture trees and nothing below the daemon's main reads the real
//! `/proc` by itself.

use super::{Caller, CallerRole, CallerTable};
use porter_core::{AppId, CgroupPath, PeerFacts, PeerIdentity, SandboxFacts, identity_of};
use std::io::ErrorKind;
use std::path::Path;

/// The caller that is process `pid` under `proc_root`, or `None` for a process nothing names.
///
/// - A native process whose executable the table names is that row's caller, wherever it was
///   launched from (a daemon under its own unit, a program started from a terminal).
/// - A Flatpak process is the app its `.flatpak-info` names, with the role of the table row for
///   that app, `App` otherwise.
/// - Any other native process is the app its launcher's `app-<id>-*.scope` names, as an `App`.
/// - A process with none of these, or a sandbox whose info cannot be read, is nobody.
pub(super) fn caller_of_pid(proc_root: &Path, pid: u32, table: &CallerTable) -> Option<Caller> {
    let dir = proc_root.join(pid.to_string());
    let sandbox = sandbox_facts(&dir)?;
    if sandbox == SandboxFacts::Native {
        // A replaced binary reads back with " (deleted)" appended, which no row names.
        if let Some(caller) = std::fs::read_link(dir.join("exe"))
            .ok()
            .and_then(|exe| table.resolve(&exe))
        {
            return Some(caller);
        }
    }
    let cgroup = std::fs::read_to_string(dir.join("cgroup"))
        .ok()
        .and_then(|text| CgroupPath::from_proc_cgroup(&text).ok());
    let facts = PeerFacts {
        pid,
        cgroup: cgroup.or_else(|| CgroupPath::parse("/").ok())?,
        sandbox: sandbox.clone(),
    };
    match identity_of(&facts) {
        PeerIdentity::Proven(app) => Some(Caller {
            role: role_of(&app, &sandbox, table),
            app,
        }),
        PeerIdentity::Unproven => None,
    }
}

/// A sandboxed app's role is its table row's; a native app's comes from its executable alone,
/// since a scope's name is the launcher's word and an executable row is the table's.
fn role_of(app: &AppId, sandbox: &SandboxFacts, table: &CallerTable) -> CallerRole {
    match sandbox {
        SandboxFacts::Native => CallerRole::App,
        _ => table.role_of(&app.name),
    }
}

/// What the process's sandbox says: `Native` without a `.flatpak-info` in its root, the facts of
/// one that parses, `None` for one that is there and cannot be read or understood.
fn sandbox_facts(dir: &Path) -> Option<SandboxFacts> {
    match std::fs::read_to_string(dir.join("root/.flatpak-info")) {
        Ok(text) => flatpak_facts(&text),
        Err(e) if e.kind() == ErrorKind::NotFound => Some(SandboxFacts::Native),
        Err(_) => None,
    }
}

/// `[Application] name` and `[Instance] instance-id` of a `.flatpak-info` (key-file text).
fn flatpak_facts(text: &str) -> Option<SandboxFacts> {
    let mut section = "";
    let (mut app, mut instance) = (None, None);
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name;
        } else if let Some((key, value)) = line.split_once('=') {
            match (section, key.trim()) {
                ("Application", "name") => app = Some(value.trim().to_owned()),
                ("Instance", "instance-id") => instance = Some(value.trim().to_owned()),
                _ => {}
            }
        }
    }
    Some(SandboxFacts::Flatpak {
        app: app?,
        instance: instance.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests;
