//! Which system caller table a daemon reads.
//!
//! The rows that give a fixed role (Settings, the sheet host, a porter daemon) come only from the
//! system's table, `/etc/porter/callers.toml`, which a person cannot write. A jailed acceptance
//! test cannot write it either, so a test build may be told to read another file instead. The
//! switch is gated as the proc root is ([`ProcGate`]): a build with the daemon's
//! `test-proc-root` feature honours [`SYSTEM_CALLERS_VAR`], every other build ignores it and says
//! so once at start. An empty value is the same as no value.

use crate::ProcGate;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The system's caller table.
pub const SYSTEM_CALLERS: &str = "/etc/porter/callers.toml";

/// The variable that names a file to read in place of [`SYSTEM_CALLERS`] (a test build only).
pub const SYSTEM_CALLERS_VAR: &str = "PORTER_SYSTEM_CALLERS";

/// Which file a daemon reads as the system's caller table.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SystemCallers {
    /// [`SYSTEM_CALLERS`].
    System,
    /// A test's file. Only a build that honours the variable gets this.
    Fake(PathBuf),
    /// The variable was set, but this build ignores it: [`SYSTEM_CALLERS`] is read.
    Ignored(PathBuf),
}

impl SystemCallers {
    /// The choice for [`SYSTEM_CALLERS_VAR`], which `lookup` finds (the daemon passes how it
    /// reads its environment; a test passes a table), under `gate`.
    ///
    /// The library reads no environment variable itself.
    pub fn choose(gate: ProcGate, lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        let value = lookup(SYSTEM_CALLERS_VAR).filter(|value| !value.is_empty());
        match (gate, value) {
            (_, None) => Self::System,
            (ProcGate::Honour, Some(file)) => Self::Fake(PathBuf::from(file)),
            (ProcGate::Ignore, Some(file)) => Self::Ignored(PathBuf::from(file)),
        }
    }

    /// The file to read.
    pub fn path(&self) -> &Path {
        match self {
            Self::Fake(file) => file,
            Self::System | Self::Ignored(_) => Path::new(SYSTEM_CALLERS),
        }
    }

    /// The line for standard error at start, when the variable was set: that a test's table is
    /// being read, or that the variable was ignored. `daemon` is the program's name, which
    /// begins the line.
    pub fn notice(&self, daemon: &str) -> Option<String> {
        match self {
            Self::System => None,
            Self::Fake(file) => Some(format!(
                "{daemon}: test system callers {}: read in place of {SYSTEM_CALLERS}",
                file.display()
            )),
            Self::Ignored(file) => Some(format!(
                "{daemon}: {SYSTEM_CALLERS_VAR}={} ignored: not a test-proc-root build",
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
            (ProcGate::Honour, None, SystemCallers::System),
            (ProcGate::Honour, Some(""), SystemCallers::System),
            (
                ProcGate::Honour,
                Some("/jail/callers.toml"),
                SystemCallers::Fake(file()),
            ),
            (ProcGate::Ignore, None, SystemCallers::System),
            (ProcGate::Ignore, Some(""), SystemCallers::System),
            (
                ProcGate::Ignore,
                Some("/jail/callers.toml"),
                SystemCallers::Ignored(file()),
            ),
        ];
        for (gate, value, expected) in cases {
            let got = SystemCallers::choose(gate, |name| {
                assert_eq!(name, SYSTEM_CALLERS_VAR);
                value.map(OsString::from)
            });
            assert_eq!(got, expected, "{gate:?} {value:?}");
            let reads_test_table = gate == ProcGate::Honour && value.is_some_and(|v| !v.is_empty());
            assert_eq!(got.path() == file(), reads_test_table, "{gate:?} {value:?}");
            assert_eq!(
                got.notice("myd").is_some(),
                value.is_some_and(|v| !v.is_empty())
            );
        }
    }

    #[test]
    fn the_notice_names_the_program_and_the_variable() {
        let ignored = SystemCallers::Ignored(PathBuf::from("/x"));
        assert_eq!(
            ignored.notice("myd").expect("line"),
            "myd: PORTER_SYSTEM_CALLERS=/x ignored: not a test-proc-root build"
        );
        assert_eq!(ignored.path(), Path::new(SYSTEM_CALLERS));
    }
}
