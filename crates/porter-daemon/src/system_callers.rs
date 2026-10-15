//! Which system caller table a daemon reads.
//!
//! The rows that give a fixed role (Settings, the sheet host, a porter daemon, cuad, the agent
//! launcher) come only from a system table under `/etc/porter`, which a person cannot write:
//! accountd and syncd read [`CALLERS`], inferd reads [`INFERD_CALLERS`]. A jailed acceptance
//! test cannot write `/etc` either, so a test build may be told to read another file instead.
//! The switch is gated as the proc root is ([`ProcGate`]): a build with the daemon's
//! `test-proc-root` feature honours the file's variable, every other build ignores it and says so
//! once at start. An empty value is the same as no value.

use crate::ProcGate;
use std::ffi::OsString;
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
        let value = lookup(file.var).filter(|value| !value.is_empty());
        let choice = match (gate, value) {
            (_, None) => TableChoice::System,
            (ProcGate::Honour, Some(path)) => TableChoice::Fake(PathBuf::from(path)),
            (ProcGate::Ignore, Some(path)) => TableChoice::Ignored(PathBuf::from(path)),
        };
        Self { file, choice }
    }

    /// What the variable did.
    pub fn choice(&self) -> &TableChoice {
        &self.choice
    }

    /// The file to read.
    pub fn path(&self) -> &Path {
        match &self.choice {
            TableChoice::Fake(path) => path,
            TableChoice::System | TableChoice::Ignored(_) => Path::new(self.file.path),
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
                let got = SystemCallers::choose_file(system, gate, |name| {
                    assert_eq!(name, system.var);
                    value.map(OsString::from)
                });
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
}
