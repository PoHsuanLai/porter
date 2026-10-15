//! The XDG base directories a daemon keeps its files under, resolved from an environment the
//! caller hands in (a closure, so a test names its own directories and nothing here reads the
//! process's environment). One reader for accountd, syncd and inferd.
//!
//! The XDG rule: a variable whose value is not an absolute path is invalid and ignored, as if it
//! were not set; the directory then falls back to its place under `$HOME`, and a `$HOME` that is
//! not absolute is no home.

use std::path::{Path, PathBuf};

/// Why a directory could not be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PathError {
    /// Neither `HOME` nor the XDG variable of a directory is set to an absolute path.
    #[error("HOME is not set and no XDG directory names where to keep state")]
    NoHome,
}

/// The absolute path `value` names; `None` for no value, an empty one or a relative one.
pub fn absolute(value: Option<String>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|p| p.is_absolute())
}

/// An environment, read through `var`, and the home directory it names.
#[derive(Debug, Clone)]
pub struct Xdg<F> {
    var: F,
    home: Option<PathBuf>,
}

impl<F: Fn(&str) -> Option<String>> Xdg<F> {
    /// The environment `var` answers (`HOME` and the XDG variables).
    pub fn new(var: F) -> Self {
        let home = absolute(var("HOME"));
        Self { var, home }
    }

    /// The directory the variable `name` names, or `fallback` below the home directory.
    ///
    /// # Errors
    /// The variable is unset (or not absolute) and there is no home to place `fallback` in.
    pub fn dir(&self, name: &str, fallback: &str) -> Result<PathBuf, PathError> {
        match (absolute((self.var)(name)), &self.home) {
            (Some(dir), _) => Ok(dir),
            (None, Some(home)) => Ok(home.join(fallback)),
            (None, None) => Err(PathError::NoHome),
        }
    }

    /// The absolute path the variable `name` names, if it names one (no fallback).
    pub fn var_dir(&self, name: &str) -> Option<PathBuf> {
        absolute((self.var)(name))
    }

    /// The home directory, when `HOME` is an absolute path.
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    /// The raw value of the variable `name`.
    pub fn raw(&self, name: &str) -> Option<String> {
        (self.var)(name)
    }

    /// The system data directories, most important first: each absolute `$XDG_DATA_DIRS`
    /// entry, or [`DEFAULT_DATA_DIRS`] when it is unset or empty.
    pub fn data_dirs(&self) -> Vec<PathBuf> {
        search_list((self.var)("XDG_DATA_DIRS"), DEFAULT_DATA_DIRS)
    }

    /// The system configuration directories, most important first: each absolute
    /// `$XDG_CONFIG_DIRS` entry, or `/etc/xdg`.
    pub fn config_dirs(&self) -> Vec<PathBuf> {
        search_list((self.var)("XDG_CONFIG_DIRS"), DEFAULT_CONFIG_DIRS)
    }

    /// Where the system's copy of `relative` (`porter/providers`, `stoker/catalog`, ...) is: under
    /// the first data directory that has it, so a package installed under `/usr/local` is found
    /// as one under `/usr` is; `/usr/share/<relative>` when none has it.
    pub fn shipped(&self, relative: &str) -> PathBuf {
        self.shipped_where(relative, Path::exists)
    }

    /// [`Xdg::shipped`], with `exists` telling which paths are there.
    pub fn shipped_where(&self, relative: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
        self.data_dirs()
            .into_iter()
            .map(|dir| dir.join(relative))
            .find(|path| exists(path))
            .unwrap_or_else(|| Path::new(SHIPPED_FALLBACK).join(relative))
    }
}

/// `$XDG_DATA_DIRS` when it is unset or empty.
pub const DEFAULT_DATA_DIRS: &str = "/usr/local/share:/usr/share";

/// `$XDG_CONFIG_DIRS` when it is unset or empty.
pub const DEFAULT_CONFIG_DIRS: &str = "/etc/xdg";

/// The data directory a shipped file is named under when no data directory has it.
const SHIPPED_FALLBACK: &str = "/usr/share";

/// The absolute entries of a colon-separated list, or of `default` when it is unset or empty.
fn search_list(value: Option<String>, default: &str) -> Vec<PathBuf> {
    value
        .filter(|list| !list.is_empty())
        .as_deref()
        .unwrap_or(default)
        .split(':')
        .filter_map(|dir| absolute(Some(dir.to_owned())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn a_directory_is_the_variable_or_its_place_below_home() {
        let xdg = Xdg::new(env(&[("HOME", "/home/ada"), ("XDG_STATE_HOME", "/state")]));
        assert_eq!(
            xdg.dir("XDG_STATE_HOME", ".local/state"),
            Ok("/state".into())
        );
        assert_eq!(
            xdg.dir("XDG_DATA_HOME", ".local/share"),
            Ok("/home/ada/.local/share".into())
        );
        assert_eq!(xdg.home(), Some(Path::new("/home/ada")));
    }

    #[test]
    fn a_relative_or_empty_value_is_ignored() {
        let xdg = Xdg::new(env(&[
            ("HOME", "/home/ada"),
            ("XDG_CONFIG_HOME", "rel/config"),
            ("XDG_DATA_HOME", ""),
        ]));
        assert_eq!(
            xdg.dir("XDG_CONFIG_HOME", ".config"),
            Ok("/home/ada/.config".into())
        );
        assert_eq!(
            xdg.dir("XDG_DATA_HOME", ".local/share"),
            Ok("/home/ada/.local/share".into())
        );
        assert_eq!(xdg.var_dir("XDG_CONFIG_HOME"), None);
    }

    #[test]
    fn no_home_and_no_variable_is_an_error_but_a_variable_alone_is_enough() {
        let none = Xdg::new(env(&[]));
        assert_eq!(
            none.dir("XDG_STATE_HOME", ".local/state"),
            Err(PathError::NoHome)
        );
        let relative_home = Xdg::new(env(&[("HOME", "home/ada")]));
        assert_eq!(relative_home.home(), None);
        assert_eq!(
            relative_home.dir("XDG_STATE_HOME", ".local/state"),
            Err(PathError::NoHome)
        );
        let alone = Xdg::new(env(&[("XDG_STATE_HOME", "/s")]));
        assert_eq!(alone.dir("XDG_STATE_HOME", ".local/state"), Ok("/s".into()));
    }

    #[test]
    fn the_system_directories_are_the_lists_absolute_entries_or_the_defaults() {
        let unset = Xdg::new(env(&[]));
        assert_eq!(
            unset.data_dirs(),
            [PathBuf::from("/usr/local/share"), "/usr/share".into()]
        );
        assert_eq!(unset.config_dirs(), [PathBuf::from("/etc/xdg")]);
        let set = Xdg::new(env(&[
            ("XDG_DATA_DIRS", "/opt/quire/share:relative:/usr/share"),
            ("XDG_CONFIG_DIRS", ""),
        ]));
        assert_eq!(
            set.data_dirs(),
            [PathBuf::from("/opt/quire/share"), "/usr/share".into()]
        );
        assert_eq!(set.config_dirs(), [PathBuf::from("/etc/xdg")]);
    }

    #[test]
    fn a_shipped_file_is_under_the_first_data_directory_that_has_it() {
        let xdg = Xdg::new(env(&[]));
        let only = |there: &'static str| move |path: &Path| path == Path::new(there);
        assert_eq!(
            xdg.shipped_where(
                "porter/providers",
                only("/usr/local/share/porter/providers")
            ),
            PathBuf::from("/usr/local/share/porter/providers")
        );
        assert_eq!(
            xdg.shipped_where("porter/providers", only("/usr/share/porter/providers")),
            PathBuf::from("/usr/share/porter/providers")
        );
        // Nowhere: named under /usr/share, as before.
        assert_eq!(
            xdg.shipped_where("stoker/catalog", |_| false),
            PathBuf::from("/usr/share/stoker/catalog")
        );
    }
}
