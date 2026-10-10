//! Which `/proc` a daemon reads its callers from.
//!
//! A daemon decides what a caller may do by the process behind it (its cgroup under
//! `/proc/<pid>`). An acceptance test runs processes outside any unit, so a test build may be told
//! to read a fixture tree instead (`<dir>/<pid>/cgroup`). That switch is the one way to make a
//! daemon believe a caller is somebody else, so it is gated by the build and not by the
//! environment alone:
//!
//! - a build with the daemon's `test-proc-root` feature honours the variable ([`ProcGate::Honour`]);
//! - every other build ignores it ([`ProcGate::Ignore`]), and says so once at start, so a person
//!   who set it by mistake is told it did nothing;
//! - an empty value is the same as no value.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Whether this build honours the proc-root variable. The daemon decides from its own feature:
/// `ProcGate::Honour` in a build with `test-proc-root`, `ProcGate::Ignore` in every other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProcGate {
    /// The variable names the process tree.
    Honour,
    /// The variable is ignored.
    Ignore,
}

/// Where a daemon reads its callers' processes from.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProcRoot {
    /// The system's `/proc`.
    System,
    /// A fixture tree: `<dir>/<pid>/cgroup`. Only a build that honours the variable gets this.
    Fake(PathBuf),
    /// The variable was set, but this build ignores it: the system's `/proc` is read.
    Ignored(PathBuf),
}

impl ProcRoot {
    /// The choice for the variable `name`, which `lookup` finds (the daemon passes how it reads
    /// its environment; a test passes a table), under `gate`.
    ///
    /// The library reads no environment variable itself.
    pub fn choose(gate: ProcGate, name: &str, lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        let value = lookup(name).filter(|value| !value.is_empty());
        match (gate, value) {
            (_, None) => Self::System,
            (ProcGate::Honour, Some(dir)) => Self::Fake(PathBuf::from(dir)),
            (ProcGate::Ignore, Some(dir)) => Self::Ignored(PathBuf::from(dir)),
        }
    }

    /// The directory to read callers from: the fixture tree, or `/proc`.
    pub fn path(&self) -> &Path {
        match self {
            Self::Fake(dir) => dir,
            Self::System | Self::Ignored(_) => Path::new("/proc"),
        }
    }

    /// The fixture tree, when this build honoured the variable; `None` means the system's
    /// `/proc`.
    pub fn fixture(&self) -> Option<&Path> {
        match self {
            Self::Fake(dir) => Some(dir),
            Self::System | Self::Ignored(_) => None,
        }
    }

    /// As [`ProcRoot::fixture`], taking the choice.
    pub fn into_fixture(self) -> Option<PathBuf> {
        match self {
            Self::Fake(dir) => Some(dir),
            Self::System | Self::Ignored(_) => None,
        }
    }

    /// The line for standard error at start, when the variable `var` was set: that a fixture is
    /// being read, or that the variable was ignored. `daemon` is the program's name, which
    /// begins the line.
    pub fn notice(&self, daemon: &str, var: &str) -> Option<String> {
        match self {
            Self::System => None,
            Self::Fake(dir) => Some(format!(
                "{daemon}: test proc root {}: callers are read from it, not /proc",
                dir.display()
            )),
            Self::Ignored(dir) => Some(format!(
                "{daemon}: {var}={} ignored: not a test-proc-root build",
                dir.display()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VAR: &str = "MYD_PROC_ROOT";

    #[test]
    fn only_a_build_that_honours_the_variable_reads_the_fixture() {
        let dir = || PathBuf::from("/fake");
        let cases = [
            (ProcGate::Honour, None, ProcRoot::System),
            (ProcGate::Honour, Some(""), ProcRoot::System),
            (ProcGate::Honour, Some("/fake"), ProcRoot::Fake(dir())),
            (ProcGate::Ignore, None, ProcRoot::System),
            (ProcGate::Ignore, Some(""), ProcRoot::System),
            (ProcGate::Ignore, Some("/fake"), ProcRoot::Ignored(dir())),
        ];
        for (gate, value, expected) in cases {
            let got = ProcRoot::choose(gate, VAR, |name| {
                assert_eq!(name, VAR);
                value.map(OsString::from)
            });
            assert_eq!(got, expected, "{gate:?} {value:?}");
            let said = got.notice("myd", VAR);
            assert_eq!(said.is_some(), value.is_some_and(|v| !v.is_empty()));
            let reads_fixture = got.fixture().is_some();
            assert_eq!(reads_fixture, gate == ProcGate::Honour && said.is_some());
            assert_eq!(
                got.path() == Path::new("/fake"),
                reads_fixture,
                "{gate:?} {value:?}"
            );
        }
    }

    #[test]
    fn the_notice_names_the_program_and_the_variable() {
        let ignored = ProcRoot::Ignored(PathBuf::from("/x"));
        let line = ignored.notice("myd", VAR).expect("line");
        assert_eq!(
            line,
            "myd: MYD_PROC_ROOT=/x ignored: not a test-proc-root build"
        );
        let fake = ProcRoot::Fake(PathBuf::from("/x"));
        assert_eq!(
            fake.notice("myd", VAR).expect("line"),
            "myd: test proc root /x: callers are read from it, not /proc"
        );
        assert_eq!(fake.into_fixture(), Some(PathBuf::from("/x")));
    }
}
