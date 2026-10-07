//! Where accountd reads and writes, resolved from the environment (PLAN §2.3, §2.6): the one
//! place the daemon names a path, so a test can give it scratch directories.

use std::path::{Path, PathBuf};

/// Every path the daemon uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The directory of `registry.json`: `$XDG_STATE_HOME/porter`.
    pub registry_dir: PathBuf,
    /// `$XDG_STATE_HOME/quire/accountd/audit.jsonl`.
    pub audit: PathBuf,
    /// The system caller table, `/etc/porter/callers.toml`.
    pub callers_system: PathBuf,
    /// The user's caller table, `$XDG_CONFIG_HOME/porter/callers.toml`; its rows win.
    pub callers_user: PathBuf,
    /// The daemon's config with its `[adopt]` table, `/etc/porter/accountd.toml`. Only the
    /// system's: a user file could let any app read the legacy store.
    pub config: PathBuf,
    /// The shipped clients, `/usr/share/porter/clients.toml`.
    pub clients_shipped: PathBuf,
    /// The user's clients, `$XDG_CONFIG_HOME/porter/clients.toml` (Settings writes it).
    pub clients_user: PathBuf,
    /// Provider file directories, later ones winning: the system's, then the user's.
    pub provider_dirs: Vec<PathBuf>,
    /// The `applications` directories that hold desktop entries, the person's first:
    /// `$XDG_DATA_HOME/applications`, then each absolute `$XDG_DATA_DIRS` entry's (default
    /// `/usr/local/share:/usr/share`).
    pub applications: Vec<PathBuf>,
}

/// Why paths could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// Neither `HOME` nor the XDG variable of a directory is set.
    NoHome,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HOME is not set and no XDG directory names where to keep state")
    }
}

impl std::error::Error for PathError {}

fn absolute(value: Option<String>) -> Option<PathBuf> {
    // The XDG rule: a relative path in these variables is invalid and ignored.
    value.map(PathBuf::from).filter(|p| p.is_absolute())
}

impl Paths {
    /// The paths for the environment `var` reads, with `extra_providers` laid over the system's
    /// and the user's provider directories.
    pub fn resolve(
        var: impl Fn(&str) -> Option<String>,
        extra_providers: &[PathBuf],
    ) -> Result<Self, PathError> {
        let home = absolute(var("HOME"));
        let under = |name: &str, tail: &str| match (absolute(var(name)), &home) {
            (Some(dir), _) => Ok(dir),
            (None, Some(home)) => Ok(home.join(tail)),
            (None, None) => Err(PathError::NoHome),
        };
        let state = under("XDG_STATE_HOME", ".local/state")?;
        let config = under("XDG_CONFIG_HOME", ".config")?;
        let data = under("XDG_DATA_HOME", ".local/share")?;
        let mut provider_dirs = vec![
            Path::new("/usr/share/porter/providers").to_owned(),
            data.join("porter/providers"),
        ];
        provider_dirs.extend_from_slice(extra_providers);
        let data_dirs = var("XDG_DATA_DIRS")
            .filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
        let applications = std::iter::once(data.clone())
            .chain(
                data_dirs
                    .split(':')
                    .filter_map(|dir| absolute(Some(dir.to_owned()))),
            )
            .map(|dir| dir.join("applications"))
            .collect();
        Ok(Self {
            registry_dir: state.join("porter"),
            audit: state.join("quire/accountd/audit.jsonl"),
            callers_system: PathBuf::from("/etc/porter/callers.toml"),
            callers_user: config.join("porter/callers.toml"),
            config: PathBuf::from("/etc/porter/accountd.toml"),
            clients_shipped: PathBuf::from("/usr/share/porter/clients.toml"),
            clients_user: config.join("porter/clients.toml"),
            provider_dirs,
            applications,
        })
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
    fn home_alone_gives_the_xdg_defaults() {
        let paths = Paths::resolve(env(&[("HOME", "/home/ada")]), &[]).expect("paths");
        assert_eq!(
            paths.registry_dir,
            Path::new("/home/ada/.local/state/porter")
        );
        assert_eq!(
            paths.audit,
            Path::new("/home/ada/.local/state/quire/accountd/audit.jsonl")
        );
        assert_eq!(
            paths.callers_user,
            Path::new("/home/ada/.config/porter/callers.toml")
        );
        assert_eq!(
            paths.provider_dirs,
            vec![
                PathBuf::from("/usr/share/porter/providers"),
                PathBuf::from("/home/ada/.local/share/porter/providers")
            ]
        );
    }

    #[test]
    fn xdg_variables_win_and_relative_ones_are_ignored() {
        let paths = Paths::resolve(
            env(&[
                ("HOME", "/home/ada"),
                ("XDG_STATE_HOME", "/scratch/state"),
                ("XDG_CONFIG_HOME", "relative/config"),
            ]),
            &[PathBuf::from("/extra")],
        )
        .expect("paths");
        assert_eq!(paths.registry_dir, Path::new("/scratch/state/porter"));
        assert_eq!(
            paths.clients_user,
            Path::new("/home/ada/.config/porter/clients.toml")
        );
        assert_eq!(paths.provider_dirs.last(), Some(&PathBuf::from("/extra")));
    }

    #[test]
    fn desktop_entries_are_looked_for_in_the_data_home_then_the_data_dirs() {
        let defaults = Paths::resolve(env(&[("HOME", "/home/ada")]), &[]).expect("paths");
        assert_eq!(
            defaults.applications,
            [
                "/home/ada/.local/share/applications",
                "/usr/local/share/applications",
                "/usr/share/applications"
            ]
            .map(PathBuf::from)
        );
        let set = Paths::resolve(
            env(&[
                ("HOME", "/home/ada"),
                ("XDG_DATA_HOME", "/scratch/data"),
                ("XDG_DATA_DIRS", "/one:relative:/two"),
            ]),
            &[],
        )
        .expect("paths");
        assert_eq!(
            set.applications,
            [
                "/scratch/data/applications",
                "/one/applications",
                "/two/applications"
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn no_home_and_no_xdg_is_an_error() {
        assert_eq!(Paths::resolve(env(&[]), &[]), Err(PathError::NoHome));
        assert!(
            Paths::resolve(
                env(&[
                    ("XDG_STATE_HOME", "/s"),
                    ("XDG_CONFIG_HOME", "/c"),
                    ("XDG_DATA_HOME", "/d")
                ]),
                &[]
            )
            .is_ok()
        );
    }
}

/// Which build this is, for the one knob a release must not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    /// Built with the `test-proc-root` feature.
    Test,
    /// Every other build: the shipped one.
    Release,
}

/// The build this binary is.
pub const BUILD: Build = if cfg!(feature = "test-proc-root") {
    Build::Test
} else {
    Build::Release
};

/// The directory callers are read from in place of `/proc` (`ACCOUNTD_PROC_ROOT`), for a test
/// build only: the jailed and acceptance tests run processes outside service cgroups. A release
/// build ignores the variable.
pub fn proc_root(build: Build, value: Option<String>) -> Option<PathBuf> {
    match build {
        Build::Test => value.filter(|v| !v.is_empty()).map(PathBuf::from),
        Build::Release => None,
    }
}

#[cfg(test)]
mod proc_root_tests {
    use super::*;

    #[test]
    fn only_a_test_build_honours_the_proc_root_variable() {
        let set = Some("/fixture/proc".to_owned());
        assert_eq!(
            proc_root(Build::Test, set.clone()),
            Some(PathBuf::from("/fixture/proc"))
        );
        assert_eq!(proc_root(Build::Test, Some(String::new())), None);
        assert_eq!(proc_root(Build::Test, None), None);
        assert_eq!(proc_root(Build::Release, set), None);
    }

    #[test]
    fn a_shipped_build_is_a_release_build() {
        let manifest = include_str!("../Cargo.toml");
        let default = manifest
            .lines()
            .skip_while(|l| !l.starts_with("[features]"))
            .any(|l| l.trim_start().starts_with("default"));
        assert!(
            !default,
            "no feature is default, so test-proc-root never is"
        );
    }
}
