//! Which `/proc` a daemon reads its callers from.
//!
//! A daemon decides what a caller may do by the process behind it (its cgroup under
//! `/proc/<pid>`). An acceptance test runs processes outside any unit, so a test build may be told
//! to read a fixture tree instead (`<dir>/<pid>/cgroup`). That switch is the one way to make a
//! daemon believe a caller is somebody else, so it is gated by the build and not by the
//! environment alone:
//!
//! - a build with the daemon's `test-proc-root` feature honours the variable
//!   ([`Override::Honoured`]);
//! - every other build ignores it ([`Override::Ignored`]), and says so once at start, so a person
//!   who set it by mistake is told it did nothing;
//! - an empty value is the same as no value.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// What a variable that only a test build may use did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Override<T> {
    /// The variable was not set (or was empty): the system's own is used.
    Unset,
    /// It was set and this build honours it: `T` is used in place of the system's.
    Honoured(T),
    /// It was set, but this build ignores it: the system's own is used.
    Ignored(T),
}

impl<T> Override<T> {
    /// The choice for a variable whose non-empty value is `value`, in a build that `honour`s it
    /// or not (the daemon decides from its own feature).
    pub fn choose(honour: bool, value: Option<T>) -> Self {
        match value {
            None => Self::Unset,
            Some(value) if honour => Self::Honoured(value),
            Some(value) => Self::Ignored(value),
        }
    }

    /// The value in use in place of the system's, when this build honoured the variable.
    pub fn honoured(&self) -> Option<&T> {
        match self {
            Self::Honoured(value) => Some(value),
            Self::Unset | Self::Ignored(_) => None,
        }
    }

    /// As [`Override::honoured`], taking the choice.
    pub fn into_honoured(self) -> Option<T> {
        match self {
            Self::Honoured(value) => Some(value),
            Self::Unset | Self::Ignored(_) => None,
        }
    }
}

/// Where a daemon reads its callers' processes from: the fixture tree (`<dir>/<pid>/cgroup`) a
/// build that honours the variable was given, or the system's `/proc`.
pub type ProcRoot = Override<PathBuf>;

/// The choice for the variable `name`, which `lookup` finds (the daemon passes how it reads its
/// environment; a test passes a table), in a build that `honour`s it or not.
///
/// The library reads no environment variable itself.
pub fn choose_proc_root(
    honour: bool,
    name: &str,
    lookup: impl Fn(&str) -> Option<OsString>,
) -> ProcRoot {
    Override::choose(
        honour,
        lookup(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
    )
}

/// The directory to read callers from: the fixture tree, or `/proc`.
pub fn proc_path(root: &ProcRoot) -> &Path {
    root.honoured()
        .map_or_else(|| Path::new("/proc"), PathBuf::as_path)
}

/// The line for standard error at start, when the variable `var` was set: that a fixture is being
/// read, or that the variable was ignored. `daemon` is the program's name, which begins the line.
pub fn proc_notice(root: &ProcRoot, daemon: &str, var: &str) -> Option<String> {
    match root {
        Override::Unset => None,
        Override::Honoured(dir) => Some(format!(
            "{daemon}: test proc root {}: callers are read from it, not /proc",
            dir.display()
        )),
        Override::Ignored(dir) => Some(format!(
            "{daemon}: {var}={} ignored: not a test-proc-root build",
            dir.display()
        )),
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
            (true, None, ProcRoot::Unset),
            (true, Some(""), ProcRoot::Unset),
            (true, Some("/fake"), ProcRoot::Honoured(dir())),
            (false, None, ProcRoot::Unset),
            (false, Some(""), ProcRoot::Unset),
            (false, Some("/fake"), ProcRoot::Ignored(dir())),
        ];
        for (honour, value, expected) in cases {
            let got = choose_proc_root(honour, VAR, |name| {
                assert_eq!(name, VAR);
                value.map(OsString::from)
            });
            assert_eq!(got, expected, "{honour:?} {value:?}");
            let said = proc_notice(&got, "myd", VAR);
            assert_eq!(said.is_some(), value.is_some_and(|v| !v.is_empty()));
            let reads_fixture = got.honoured().is_some();
            assert_eq!(reads_fixture, honour && said.is_some());
            assert_eq!(
                proc_path(&got) == Path::new("/fake"),
                reads_fixture,
                "{honour:?} {value:?}"
            );
        }
    }

    #[test]
    fn the_notice_names_the_program_and_the_variable() {
        let ignored = ProcRoot::Ignored(PathBuf::from("/x"));
        let line = proc_notice(&ignored, "myd", VAR).expect("line");
        assert_eq!(
            line,
            "myd: MYD_PROC_ROOT=/x ignored: not a test-proc-root build"
        );
        let fake = ProcRoot::Honoured(PathBuf::from("/x"));
        assert_eq!(
            proc_notice(&fake, "myd", VAR).expect("line"),
            "myd: test proc root /x: callers are read from it, not /proc"
        );
        assert_eq!(fake.into_honoured(), Some(PathBuf::from("/x")));
    }
}
