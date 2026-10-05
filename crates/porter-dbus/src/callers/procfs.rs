//! Naming a process from its cgroup, the one file read. `/proc/<pid>/exe` and
//! `/proc/<pid>/root/.flatpak-info` are ptrace-gated and a Landlock domain blocks them, so
//! nothing here touches them. The root is a parameter, so the tests read fixture trees.

use super::{Caller, CallerTable};
use porter_core::identity_of;
use porter_core::{AppId, AppName, CgroupPath, Isolation, PeerFacts, PeerIdentity, SandboxFacts};
use std::path::Path;

/// The launcher scope Flatpak makes: `app-flatpak-<escaped id>-<n>.scope`.
const FLATPAK_PREFIX: &str = "app-flatpak-";

/// The launcher namespace of app scopes: `app-[<launcher>-]<id>-<n>.scope`.
const APP_PREFIX: &str = "app-";

/// The caller that is process `pid` under `proc_root`, or `None` for a process nothing names.
///
/// - `<name>.service`: the unit the table names, as that row's caller.
/// - `<name>.scope` that is not an app scope (`sill-shell.scope`, started by
///   `systemd-run --scope --unit=sill-shell`): the same, matched exactly, never by prefix.
/// - `app-flatpak-<id>-<n>.scope`: the Flatpak app `<id>`, with the role of its table row.
/// - `app-[<launcher>-]<id>-<n>.scope`: the native app `<id>`, with the role of its table row.
/// - Anything else (a terminal's child, a session scope, an unlisted unit, no cgroup): nobody.
pub(super) fn caller_of_pid(proc_root: &Path, pid: u32, table: &CallerTable) -> Option<Caller> {
    let text = std::fs::read_to_string(proc_root.join(pid.to_string()).join("cgroup")).ok()?;
    let cgroup = CgroupPath::from_proc_cgroup(&text).ok()?;
    let leaf = cgroup.as_str().rsplit('/').next().unwrap_or_default();
    if is_unit_leaf(leaf) {
        return table.resolve_unit(leaf);
    }
    let app = flatpak_scope_app(leaf).map(|name| AppId {
        name,
        isolation: Isolation::Flatpak,
    });
    let app = match app {
        Some(app) => app,
        None => native_app(pid, cgroup)?,
    };
    Some(Caller {
        role: table.role_of(&app.name),
        app,
    })
}

/// A leaf named as a unit: a service, or a scope outside the `app-` launcher namespace (those name
/// apps and get no unit's role).
fn is_unit_leaf(leaf: &str) -> bool {
    leaf.ends_with(".service") || (leaf.ends_with(".scope") && !leaf.starts_with(APP_PREFIX))
}

/// The app a native launcher's scope names.
fn native_app(pid: u32, cgroup: CgroupPath) -> Option<AppId> {
    let facts = PeerFacts {
        pid,
        cgroup,
        sandbox: SandboxFacts::Native,
    };
    match identity_of(&facts) {
        PeerIdentity::Proven(app) => Some(app),
        PeerIdentity::Unproven => None,
    }
}

/// The app of `app-flatpak-<escaped id>-<n>.scope`.
fn flatpak_scope_app(leaf: &str) -> Option<AppName> {
    let unit = leaf.strip_prefix(FLATPAK_PREFIX)?.strip_suffix(".scope")?;
    let (id, instance) = unit.rsplit_once('-')?;
    let numeric = !instance.is_empty() && instance.bytes().all(|b| b.is_ascii_alphanumeric());
    if !numeric {
        return None;
    }
    AppName::parse(&unescape(id)?).ok()
}

/// A unit name component with its `\xHH` escapes undone, or `None` for a broken escape.
fn unescape(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&first, tail)) = rest.split_first() {
        rest = match (first, tail) {
            (b'\\', [b'x', high, low, after @ ..]) => {
                let digit = |b: u8| {
                    char::from(b)
                        .to_digit(16)
                        .and_then(|d| u8::try_from(d).ok())
                };
                bytes.push((digit(*high)? << 4) | digit(*low)?);
                after
            }
            (b'\\', _) => return None,
            _ => {
                bytes.push(first);
                tail
            }
        };
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests;
