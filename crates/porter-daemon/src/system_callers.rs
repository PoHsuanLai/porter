//! Which system caller table a daemon reads.
//!
//! The rows that give a fixed role (Settings, the sheet host, a porter daemon, cuad, the agent
//! launcher) come only from a system table under `/etc/porter`, which a person cannot write:
//! accountd and syncd read [`CALLERS`], inferd reads [`INFERD_CALLERS`]. A jailed acceptance
//! test cannot write `/etc` either, so a test build may be told to read another file instead.
//! The switch is gated as the proc root is ([`ProcGate`]): a build with the daemon's
//! `test-proc-root` feature honours the file's variable, every other build ignores it and says so
//! once at start. An empty value is the same as no value.
//!
//! A system table may be installed somewhere other than `/etc/porter` (a bundle under
//! `/opt/quire` ships its own): without the variable, a daemon reads the first of
//! `/etc/porter/<file>`, `<dir>/porter/<file>` for each `$XDG_CONFIG_DIRS` entry, then the same
//! for each `$XDG_DATA_DIRS` entry, that root holds ([`root_held`]). Those lists come from the
//! person's environment, so a table counts only when the person could not have written it.

use crate::ProcGate;
use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The system's caller table, read by accountd and syncd.
pub const SYSTEM_CALLERS: &str = "/etc/porter/callers.toml";

/// The variable that names a file to read in place of [`SYSTEM_CALLERS`] (a test build only).
pub const SYSTEM_CALLERS_VAR: &str = "PORTER_SYSTEM_CALLERS";

/// A system table and the variable a test build may name another file with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemFile {
    /// The file a shipped build reads.
    pub path: &'static str,
    /// The variable that names a test's file in its place.
    pub var: &'static str,
}

/// accountd's and syncd's: [`SYSTEM_CALLERS`], [`SYSTEM_CALLERS_VAR`].
pub const CALLERS: SystemFile = SystemFile {
    path: SYSTEM_CALLERS,
    var: SYSTEM_CALLERS_VAR,
};

/// inferd's caller table (its cua, Settings, shell, agent launcher, places and app units).
pub const INFERD_CALLERS: SystemFile = SystemFile {
    path: "/etc/porter/inferd-callers.toml",
    var: "PORTER_INFERD_CALLERS",
};

/// Which file a daemon reads as a system table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemCallers {
    file: SystemFile,
    choice: TableChoice,
    /// The system's file the search found (`file.path` when it found none).
    system: PathBuf,
}

/// What the variable did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TableChoice {
    /// The system's file.
    System,
    /// A test's file. Only a build that honours the variable gets this.
    Fake(PathBuf),
    /// The variable was set, but this build ignores it: the system's file is read.
    Ignored(PathBuf),
}

impl SystemCallers {
    /// The choice for [`CALLERS`] under `gate`; see [`SystemCallers::choose_file`].
    pub fn choose(gate: ProcGate, lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        Self::choose_file(CALLERS, gate, lookup)
    }

    /// The choice for `file`'s variable, which `lookup` finds (the daemon passes how it reads
    /// its environment; a test passes a table), under `gate`.
    ///
    /// The library reads no environment variable itself.
    pub fn choose_file(
        file: SystemFile,
        gate: ProcGate,
        lookup: impl Fn(&str) -> Option<OsString>,
    ) -> Self {
        Self::choose_where(file, gate, lookup, root_held)
    }

    /// [`SystemCallers::choose_file`], with `held` telling which files root holds.
    pub fn choose_where(
        file: SystemFile,
        gate: ProcGate,
        lookup: impl Fn(&str) -> Option<OsString>,
        held: impl Fn(&Path) -> bool,
    ) -> Self {
        let value = lookup(file.var).filter(|value| !value.is_empty());
        let choice = match (gate, value) {
            (_, None) => TableChoice::System,
            (ProcGate::Honour, Some(path)) => TableChoice::Fake(PathBuf::from(path)),
            (ProcGate::Ignore, Some(path)) => TableChoice::Ignored(PathBuf::from(path)),
        };
        let system = match choice {
            TableChoice::Fake(_) => PathBuf::from(file.path),
            TableChoice::System | TableChoice::Ignored(_) => search(file, &lookup, held),
        };
        Self {
            file,
            choice,
            system,
        }
    }

    /// What the variable did.
    pub fn choice(&self) -> &TableChoice {
        &self.choice
    }

    /// The file to read.
    pub fn path(&self) -> &Path {
        match &self.choice {
            TableChoice::Fake(path) => path,
            TableChoice::System | TableChoice::Ignored(_) => &self.system,
        }
    }

    /// The line for standard error at start, when the variable was set: that a test's table is
    /// being read, or that the variable was ignored. `daemon` is the program's name, which
    /// begins the line.
    pub fn notice(&self, daemon: &str) -> Option<String> {
        let SystemFile { path, var } = self.file;
        match &self.choice {
            TableChoice::System => None,
            TableChoice::Fake(file) => Some(format!(
                "{daemon}: test system callers {}: read in place of {path}",
                file.display()
            )),
            TableChoice::Ignored(file) => Some(format!(
                "{daemon}: {var}={} ignored: not a test-proc-root build",
                file.display()
            )),
        }
    }
}

/// The first place `file` is installed that root holds, else `file.path`.
fn search(
    file: SystemFile,
    lookup: impl Fn(&str) -> Option<OsString>,
    held: impl Fn(&Path) -> bool,
) -> PathBuf {
    let primary = Path::new(file.path);
    let Some(name) = primary.file_name() else {
        return primary.to_owned();
    };
    let list = |var: &str, default: &str| -> Vec<PathBuf> {
        let value = lookup(var).filter(|value| !value.is_empty());
        let text = value
            .as_ref()
            .map_or(default.into(), |v| v.to_string_lossy());
        text.split(':')
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .collect()
    };
    let elsewhere = list("XDG_CONFIG_DIRS", "/etc/xdg")
        .into_iter()
        .chain(list("XDG_DATA_DIRS", "/usr/local/share:/usr/share"))
        .map(|dir| dir.join("porter").join(name));
    std::iter::once(primary.to_owned())
        .chain(elsewhere)
        .find(|path| held(path))
        .unwrap_or_else(|| primary.to_owned())
}

/// Whether `path` is a file only root can have written: it, and every directory on its real
/// path, belongs to root and is writable by nobody else.
pub fn root_held(path: &Path) -> bool {
    let Ok(real) = std::fs::canonicalize(path) else {
        return false;
    };
    let is_file = std::fs::metadata(&real).is_ok_and(|meta| meta.is_file());
    is_file
        && real.ancestors().all(|step| {
            std::fs::metadata(step).is_ok_and(|meta| meta.uid() == 0 && meta.mode() & 0o022 == 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_build_that_honours_the_variable_reads_another_table() {
        let file = || PathBuf::from("/jail/callers.toml");
        let cases = [
            (ProcGate::Honour, None, TableChoice::System),
            (ProcGate::Honour, Some(""), TableChoice::System),
            (
                ProcGate::Honour,
                Some("/jail/callers.toml"),
                TableChoice::Fake(file()),
            ),
            (ProcGate::Ignore, None, TableChoice::System),
            (ProcGate::Ignore, Some(""), TableChoice::System),
            (
                ProcGate::Ignore,
                Some("/jail/callers.toml"),
                TableChoice::Ignored(file()),
            ),
        ];
        for system in [CALLERS, INFERD_CALLERS] {
            for (gate, value, expected) in cases.clone() {
                // Nothing root holds is found by the search, so the system's path is the usual one.
                let got = SystemCallers::choose_where(
                    system,
                    gate,
                    |name| {
                        (name == system.var)
                            .then(|| value.map(OsString::from))
                            .flatten()
                    },
                    |_| false,
                );
                assert_eq!(got.choice(), &expected, "{system:?} {gate:?} {value:?}");
                let reads_test_table =
                    gate == ProcGate::Honour && value.is_some_and(|v| !v.is_empty());
                let want = if reads_test_table {
                    file()
                } else {
                    PathBuf::from(system.path)
                };
                assert_eq!(got.path(), want, "{system:?} {gate:?} {value:?}");
                assert_eq!(
                    got.notice("myd").is_some(),
                    value.is_some_and(|v| !v.is_empty())
                );
            }
        }
    }

    #[test]
    fn the_notice_names_the_program_and_the_files_own_variable() {
        let ignored = |file| {
            SystemCallers::choose_file(file, ProcGate::Ignore, |_| Some(OsString::from("/x")))
        };
        assert_eq!(
            ignored(CALLERS).notice("myd").expect("line"),
            "myd: PORTER_SYSTEM_CALLERS=/x ignored: not a test-proc-root build"
        );
        assert_eq!(
            ignored(INFERD_CALLERS).notice("inferd").expect("line"),
            "inferd: PORTER_INFERD_CALLERS=/x ignored: not a test-proc-root build"
        );
        assert_eq!(ignored(CALLERS).path(), Path::new(SYSTEM_CALLERS));
    }

    #[test]
    fn without_the_variable_the_first_table_root_holds_is_read() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        let held_only = |there: &'static str| move |path: &Path| path == Path::new(there);
        let choose = |held: &dyn Fn(&Path) -> bool, vars| {
            SystemCallers::choose_where(INFERD_CALLERS, ProcGate::Ignore, vars, held)
                .path()
                .to_owned()
        };
        // /etc/porter first, as always.
        assert_eq!(
            choose(&held_only("/etc/porter/inferd-callers.toml"), env(&[])),
            PathBuf::from("/etc/porter/inferd-callers.toml")
        );
        // A bundle's table under a data directory.
        let bundle = env(&[("XDG_DATA_DIRS", "/opt/quire/share:/usr/share")]);
        assert_eq!(
            choose(
                &held_only("/opt/quire/share/porter/inferd-callers.toml"),
                bundle
            ),
            PathBuf::from("/opt/quire/share/porter/inferd-callers.toml")
        );
        // A configuration directory comes before the data directories.
        assert_eq!(
            choose(
                &|path: &Path| path.starts_with("/etc/xdg") || path.starts_with("/usr"),
                env(&[])
            ),
            PathBuf::from("/etc/xdg/porter/inferd-callers.toml")
        );
        // Nothing root holds: the usual path, which reads as an empty table.
        assert_eq!(
            choose(
                &|_: &Path| false,
                env(&[("XDG_CONFIG_DIRS", "/home/u/evil")])
            ),
            PathBuf::from("/etc/porter/inferd-callers.toml")
        );
    }

    #[test]
    fn a_table_the_person_could_have_written_is_not_root_held() {
        let dir = std::env::temp_dir().join(format!("porter-held-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("porter")).expect("dir");
        let table = dir.join("porter/callers.toml");
        std::fs::write(&table, "").expect("write");
        // Owned by whoever runs the test: refused even if that is root, since the
        // temporary directory is writable by everyone.
        assert!(!root_held(&table));
        assert!(!root_held(&dir.join("porter/absent.toml")));
        // A directory is not a table.
        assert!(!root_held(Path::new("/")));
        std::fs::remove_dir_all(&dir).ok();
    }
}
