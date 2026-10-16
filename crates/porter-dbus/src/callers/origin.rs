//! Where a unit's definition came from, before its name is trusted with a fixed role.
//!
//! The threat, in plain terms. The system's caller table says "the unit `sill.service` is the
//! shell". But the person's systemd user manager reads unit files from places the person can
//! write: `~/.config/systemd/user/`, `$XDG_RUNTIME_DIR/systemd/user/`, and their `X.service.d/`
//! drop-in directories. Any program of the person's can write `sill.service` there, run
//! `systemctl --user daemon-reload && systemctl --user restart sill.service`, and then its own
//! program is the main process of a unit called `sill.service`. The unit's *name* alone proves
//! nothing; only the file that defines it does. The same holds for the environment: systemd
//! hands every user unit the manager's environment (`systemctl --user set-environment`,
//! `~/.config/environment.d`), so `LD_PRELOAD=` set there is loaded into the real shell.
//!
//! So before a `.service` caller keeps a fixed role ([`CallerRole::is_fixed`]), porter asks the
//! manager about the unit (`GetUnit`, then `FragmentPath`, `DropInPaths`, `Transient` and
//! `InvocationID` of `org.freedesktop.systemd1.Unit`, and `UnsetEnvironment` of
//! `org.freedesktop.systemd1.Service`) and judges the answer with a [`UnitRule`]:
//!
//! - the fragment is a file directly in a trusted directory ([`TRUSTED_DIRS`]);
//! - every drop-in is in a `<unit>.d`, `service.d`, `<prefix>-.service.d` or `<template>@.service.d`
//!   directory of a trusted directory;
//! - the unit is not transient (`systemd-run`: the definition lives in the runtime directory);
//! - every one of those files, resolved through symlinks and `..`, is held by root
//!   (`porter_daemon::root_held`: every ancestor belongs to uid 0 and nobody else can write it);
//! - the unit clears every loader variable ([`LOADER_VARIABLES`], below).
//!
//! Anything else, a failed lookup or a lookup that times out included, lowers the caller to a
//! plain app (as a `.scope` is lowered), and the daemon says why once per unit on stderr.
//!
//! **The desktop must install its user units and drop-ins under `/usr/lib/systemd/user`,
//! `/usr/local/lib/systemd/user` or `/etc/systemd/user`, never in a home or runtime directory,
//! and every unit with a fixed role must carry the line given at [`LOADER_VARIABLES`]. Otherwise
//! its compositor, shell and settings lose their roles.**
//!
//! Trusted directories: systemd.unit(5), "Unit File Load Path", lists for the user manager
//! `/etc/systemd/user`, `/usr/local/lib/systemd/user` and `/usr/lib/systemd/user` as the
//! system-owned directories (the rest are `$XDG_CONFIG_HOME`, `$XDG_RUNTIME_DIR`, `/run` and
//! `$XDG_DATA_HOME`/`$XDG_DATA_DIRS`). `/usr/share/systemd/user` is read only through the
//! person's `$XDG_DATA_DIRS`, so it is not trusted here.
//!
//! The answer is cached per unit and `InvocationID`, so a restarted unit is judged again. Tests
//! never ask a real manager: [`UnitOrigins::Fixture`] reads `<dir>/units/<unit>.origin` from a
//! fixture tree (only a build that honours a proc root can reach it, see `ProcGate`), and a
//! unit with no such file is not judged there.

use super::BusConnection;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// The system-owned user unit directories (systemd.unit(5), "Unit File Load Path").
pub const TRUSTED_DIRS: [&str; 3] = [
    "/usr/lib/systemd/user",
    "/usr/local/lib/systemd/user",
    "/etc/systemd/user",
];

/// Environment variables that make a program load someone's code or plug-ins. A unit with a
/// fixed role must list every one in `UnsetEnvironment=`: systemd applies it last, after the
/// manager's environment (`set-environment`, `environment.d`) and the unit's `Environment=`
/// (systemd.exec(5), "Environment variables in spawned processes"), so no one can set them for
/// the unit, short of a drop-in, which the path rule already refuses. The line to copy:
///
/// ```text
/// UnsetEnvironment=LD_PRELOAD LD_LIBRARY_PATH LD_AUDIT GCONV_PATH GIO_EXTRA_MODULES GTK_MODULES GTK_PATH GST_PLUGIN_PATH GST_PLUGIN_SYSTEM_PATH LIBGL_DRIVERS_PATH MESA_LOADER_DRIVER_OVERRIDE __EGL_VENDOR_LIBRARY_DIRS __EGL_VENDOR_LIBRARY_FILENAMES GBM_BACKENDS_PATH LIBVA_DRIVERS_PATH VK_ICD_FILENAMES VK_DRIVER_FILES VK_ADD_DRIVER_FILES VK_LAYER_PATH VK_ADD_LAYER_PATH VK_INSTANCE_LAYERS VK_LOADER_LAYERS_ENABLE
/// ```
pub const LOADER_VARIABLES: [&str; 21] = [
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "GCONV_PATH",
    "GIO_EXTRA_MODULES",
    "GTK_MODULES",
    "GTK_PATH",
    "GST_PLUGIN_PATH",
    "GST_PLUGIN_SYSTEM_PATH",
    "LIBGL_DRIVERS_PATH",
    "MESA_LOADER_DRIVER_OVERRIDE",
    "__EGL_VENDOR_LIBRARY_DIRS",
    "__EGL_VENDOR_LIBRARY_FILENAMES",
    "GBM_BACKENDS_PATH",
    "LIBVA_DRIVERS_PATH",
    "VK_ICD_FILENAMES",
    "VK_DRIVER_FILES",
    "VK_ADD_DRIVER_FILES",
    "VK_LAYER_PATH",
    "VK_ADD_LAYER_PATH",
    "VK_INSTANCE_LAYERS",
    "VK_LOADER_LAYERS_ENABLE",
];

/// How long the manager has to answer.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

/// Where the unit's definition lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Definition {
    /// Files: a fragment (none for a unit that has no file) and its drop-ins.
    Stored {
        /// `FragmentPath`.
        fragment: Option<PathBuf>,
        /// `DropInPaths`.
        drop_ins: Vec<PathBuf>,
    },
    /// Made at run time (`systemd-run`): its definition is in the runtime directory.
    Transient,
}

/// What the manager says of a loaded unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitFacts {
    /// `InvocationID`, hex: changes when the unit is started again.
    pub invocation: String,
    /// Where it is defined.
    pub definition: Definition,
    /// `UnsetEnvironment` of the service.
    pub unset: Vec<String>,
}

/// Why a unit does not keep a fixed role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The manager did not answer, or not in time, or not with a unit.
    Unknown,
    /// A transient unit.
    Transient,
    /// A unit with no fragment file.
    NoFragment,
    /// The fragment is not a root-held file directly in a trusted directory.
    Fragment(PathBuf),
    /// The drop-in is not a root-held file in a trusted directory's drop-in directory.
    DropIn(PathBuf),
    /// `UnsetEnvironment` lacks these loader variables.
    Unset(Vec<String>),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => write!(f, "the systemd user manager did not describe the unit"),
            Self::Transient => write!(f, "it is a transient unit"),
            Self::NoFragment => write!(f, "it has no unit file"),
            Self::Fragment(p) => write!(f, "its unit file {} is not root-held in a system unit directory", p.display()),
            Self::DropIn(p) => write!(f, "its drop-in {} is not root-held in a system unit directory", p.display()),
            Self::Unset(names) => write!(f, "UnsetEnvironment= lacks {}", names.join(" ")),
        }
    }
}

/// The path rule: which directories are trusted and how a file is shown to be root's.
#[derive(Debug, Clone)]
pub struct UnitRule {
    dirs: Vec<PathBuf>,
    held: fn(&Path) -> bool,
}

impl UnitRule {
    /// The rule of [`TRUSTED_DIRS`], with `held` telling which files root holds (a daemon passes
    /// `porter_daemon::root_held`; a test passes its own).
    pub fn new(dirs: Vec<PathBuf>, held: fn(&Path) -> bool) -> Self {
        Self { dirs, held }
    }

    /// [`TRUSTED_DIRS`] and `porter_daemon::root_held`.
    pub fn system() -> Self {
        Self::new(
            TRUSTED_DIRS.iter().map(PathBuf::from).collect(),
            porter_daemon::root_held,
        )
    }

    /// Whether `facts` show the unit `unit` as defined only by system-owned files.
    pub fn judge(&self, unit: &str, facts: &UnitFacts) -> Result<(), Refusal> {
        let Definition::Stored { fragment, drop_ins } = &facts.definition else {
            return Err(Refusal::Transient);
        };
        let fragment = fragment.as_deref().ok_or(Refusal::NoFragment)?;
        self.in_dir(fragment, |_| true)
            .then_some(())
            .ok_or_else(|| Refusal::Fragment(fragment.to_owned()))?;
        if let Some(bad) = drop_ins
            .iter()
            .find(|path| !self.in_dir_d(path, unit))
        {
            return Err(Refusal::DropIn(bad.clone()));
        }
        let missing: Vec<String> = LOADER_VARIABLES
            .iter()
            .filter(|name| !facts.unset.iter().any(|set| set == *name))
            .map(|name| (*name).to_owned())
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(Refusal::Unset(missing))
        }
    }

    /// The canonical trusted directories that exist.
    fn real_dirs(&self) -> Vec<PathBuf> {
        self.dirs
            .iter()
            .filter_map(|dir| std::fs::canonicalize(dir).ok())
            .collect()
    }

    /// `path`, resolved, is root-held and directly in a trusted directory (`ok` judges the
    /// file's name).
    fn in_dir(&self, path: &Path, ok: impl Fn(&Path) -> bool) -> bool {
        let Ok(real) = std::fs::canonicalize(path) else {
            return false;
        };
        let parent = real.parent().map(Path::to_owned);
        ok(&real)
            && parent.is_some_and(|parent| self.real_dirs().contains(&parent))
            && (self.held)(&real)
    }

    /// `path`, resolved, is root-held in a drop-in directory of `unit` in a trusted directory.
    fn in_dir_d(&self, path: &Path, unit: &str) -> bool {
        let Ok(real) = std::fs::canonicalize(path) else {
            return false;
        };
        let dir = real.parent();
        let name = dir.and_then(Path::file_name).and_then(|n| n.to_str());
        let in_trusted = dir
            .and_then(Path::parent)
            .is_some_and(|up| self.real_dirs().iter().any(|d| d == up));
        in_trusted && name.is_some_and(|n| drop_in_dir_serves(n, unit)) && (self.held)(&real)
    }
}

/// Whether the directory `dir_name` holds drop-ins that systemd applies to `unit`: its own
/// `<unit>.d`, `<template>@.service.d`, `<prefix>-.service.d` for a dashed prefix, or the
/// type-wide `service.d` (systemd.unit(5), "Drop-in files").
fn drop_in_dir_serves(dir_name: &str, unit: &str) -> bool {
    let Some(stem) = dir_name.strip_suffix(".d") else {
        return false;
    };
    let template = unit
        .split_once('@')
        .and_then(|(base, rest)| rest.rsplit_once('.').map(|(_, ty)| format!("{base}@.{ty}")));
    let dashed = stem
        .strip_suffix("-.service")
        .is_some_and(|prefix| unit.starts_with(&format!("{prefix}-")));
    stem == unit || stem == "service" || template.as_deref() == Some(stem) || dashed
}

/// What the lookup found.
#[derive(Debug)]
enum Lookup {
    /// The unit's facts.
    Facts(UnitFacts),
    /// Nothing is judged: a fixture tree states no origin for the unit.
    Unjudged,
    /// The lookup failed.
    Failed,
}

/// Where unit facts come from.
#[derive(Debug, Clone)]
pub enum UnitOrigins {
    /// The systemd manager on the connection's bus, judged by the rule.
    Manager(UnitRule),
    /// A fixture tree: `<dir>/units/<unit>.origin` states the facts, judged by the rule. A unit
    /// with no such file is not judged.
    Fixture {
        /// The tree (the proc root).
        dir: PathBuf,
        /// The rule, with the test's own directories.
        rule: UnitRule,
    },
}

impl UnitOrigins {
    fn rule(&self) -> &UnitRule {
        match self {
            Self::Manager(rule) | Self::Fixture { rule, .. } => rule,
        }
    }

    async fn lookup(&self, connection: &BusConnection, unit: &str) -> Lookup {
        match self {
            Self::Fixture { dir, .. } => fixture_lookup(dir, unit),
            Self::Manager(_) => {
                match tokio::time::timeout(LOOKUP_TIMEOUT, manager_facts(connection, unit)).await {
                    Ok(Some(facts)) => Lookup::Facts(facts),
                    Ok(None) | Err(_) => Lookup::Failed,
                }
            }
        }
    }

    /// Whether the fixture tree's unit passes, for a caller that cannot wait on a bus.
    pub(super) fn judge_fixture(&self, unit: &str) -> Result<(), Refusal> {
        match (self, unit.contains('/')) {
            (Self::Fixture { dir, rule }, false) => match fixture_lookup(dir, unit) {
                Lookup::Facts(facts) => rule.judge(unit, &facts),
                Lookup::Unjudged => Ok(()),
                Lookup::Failed => Err(Refusal::Unknown),
            },
            _ => Err(Refusal::Unknown),
        }
    }
}

fn fixture_lookup(dir: &Path, unit: &str) -> Lookup {
    if unit.contains('/') {
        return Lookup::Failed;
    }
    match std::fs::read_to_string(dir.join("units").join(format!("{unit}.origin"))) {
        Ok(text) => Lookup::Facts(facts_from_text(&text)),
        Err(_) => Lookup::Unjudged,
    }
}

/// Facts as a fixture states them, one per line: `invocation=<id>`, `fragment=<path>`,
/// `drop_in=<path>` (repeated), `unset=<names, space separated>`, and `transient` alone.
fn facts_from_text(text: &str) -> UnitFacts {
    let (mut invocation, mut fragment) = (String::new(), None);
    let (mut drop_ins, mut unset, mut transient) = (Vec::new(), Vec::new(), false);
    for line in text.lines().map(str::trim) {
        match line.split_once('=') {
            Some(("invocation", v)) => invocation = v.to_owned(),
            Some(("fragment", v)) => fragment = Some(PathBuf::from(v)),
            Some(("drop_in", v)) => drop_ins.push(PathBuf::from(v)),
            Some(("unset", v)) => unset.extend(v.split_whitespace().map(str::to_owned)),
            _ if line == "transient" => transient = true,
            _ => {}
        }
    }
    let definition = if transient {
        Definition::Transient
    } else {
        Definition::Stored { fragment, drop_ins }
    };
    UnitFacts {
        invocation,
        definition,
        unset,
    }
}

/// The manager's description of the loaded unit `unit`; `None` when anything fails.
async fn manager_facts(connection: &BusConnection, unit: &str) -> Option<UnitFacts> {
    use zbus::names::InterfaceName;
    let properties = super::unit_properties(connection, unit).await?;
    let unit_if = InterfaceName::try_from("org.freedesktop.systemd1.Unit").ok()?;
    let service_if = InterfaceName::try_from("org.freedesktop.systemd1.Service").ok()?;
    let fragment: String = properties
        .get(unit_if.clone(), "FragmentPath")
        .await
        .ok()?
        .try_into()
        .ok()?;
    let drop_ins: Vec<String> = properties
        .get(unit_if.clone(), "DropInPaths")
        .await
        .ok()?
        .try_into()
        .ok()?;
    let transient: bool = properties
        .get(unit_if.clone(), "Transient")
        .await
        .ok()?
        .try_into()
        .ok()?;
    let invocation: Vec<u8> = properties
        .get(unit_if, "InvocationID")
        .await
        .ok()?
        .try_into()
        .ok()?;
    let unset: Vec<String> = properties
        .get(service_if, "UnsetEnvironment")
        .await
        .ok()?
        .try_into()
        .ok()?;
    let definition = if transient {
        Definition::Transient
    } else {
        Definition::Stored {
            fragment: Some(fragment).filter(|f| !f.is_empty()).map(PathBuf::from),
            drop_ins: drop_ins.into_iter().map(PathBuf::from).collect(),
        }
    };
    Some(UnitFacts {
        invocation: invocation.iter().map(|b| format!("{b:02x}")).collect(),
        definition,
        unset,
    })
}

/// The verdicts so far, by unit and invocation, and the units already reported.
#[derive(Debug, Default)]
pub(super) struct Guard {
    verdicts: Mutex<HashMap<(String, String), Result<(), Refusal>>>,
    reported: Mutex<HashSet<String>>,
}

impl Guard {
    /// Whether the unit `unit` may keep a fixed role; says why once per unit when it may not.
    pub(super) async fn check(
        &self,
        origins: &UnitOrigins,
        connection: &BusConnection,
        unit: &str,
    ) -> Result<(), Refusal> {
        let verdict = match origins.lookup(connection, unit).await {
            Lookup::Unjudged => return Ok(()),
            Lookup::Failed => Err(Refusal::Unknown),
            Lookup::Facts(facts) => {
                let key = (unit.to_owned(), facts.invocation.clone());
                let cached = self.verdicts.lock().ok().and_then(|v| v.get(&key).cloned());
                match cached {
                    Some(verdict) => verdict,
                    None => {
                        let verdict = origins.rule().judge(unit, &facts);
                        if let Ok(mut verdicts) = self.verdicts.lock() {
                            verdicts.insert(key, verdict.clone());
                        }
                        verdict
                    }
                }
            }
        };
        if let Err(why) = &verdict {
            self.report(unit, why);
        }
        verdict
    }

    fn report(&self, unit: &str, why: &Refusal) {
        let first = self
            .reported
            .lock()
            .is_ok_and(|mut reported| reported.insert(unit.to_owned()));
        if first {
            eprintln!("porter: callers: {unit} keeps no fixed role: {why}");
        }
    }
}

#[cfg(test)]
mod tests;
