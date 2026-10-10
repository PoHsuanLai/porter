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
}
