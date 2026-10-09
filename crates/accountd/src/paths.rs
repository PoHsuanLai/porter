//! Where accountd reads and writes, resolved from the environment (PLAN §2.3, §2.6): the one
//! place the daemon names a path, so a test can give it scratch directories.

use crate::providers::Layer;
pub use porter_core::xdg::PathError;
use porter_core::xdg::{Xdg, absolute};
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
    /// Tailscale's socket: `/var/run/tailscale/tailscaled.sock`, or the absolute path in
    /// `ACCOUNTD_TAILSCALE_SOCKET` (a Tailscale kept elsewhere, and every test of the binary,
    /// which names a scratch path so that none ever reaches the real Tailscale).
    pub tailscale_socket: PathBuf,
}

/// The variable that names Tailscale's socket.
pub const TAILSCALE_SOCKET_VAR: &str = "ACCOUNTD_TAILSCALE_SOCKET";

impl Paths {
    /// The paths for the environment `var` reads, with `extra_providers` laid over the system's
    /// and the user's provider directories.
    pub fn resolve(
        var: impl Fn(&str) -> Option<String>,
        extra_providers: &[PathBuf],
    ) -> Result<Self, PathError> {
        let data_dirs = var("XDG_DATA_DIRS");
        // Like the XDG rule: a relative path is invalid and ignored.
        let tailscale_socket = absolute(var(TAILSCALE_SOCKET_VAR))
            .unwrap_or_else(|| PathBuf::from(porter_tailscale::DEFAULT_SOCKET));
        let xdg = Xdg::new(var);
        let state = xdg.dir("XDG_STATE_HOME", ".local/state")?;
        let config = xdg.dir("XDG_CONFIG_HOME", ".config")?;
        let data = xdg.dir("XDG_DATA_HOME", ".local/share")?;
        let mut provider_dirs = vec![
            Path::new("/usr/share/porter/providers").to_owned(),
            data.join("porter/providers"),
        ];
        provider_dirs.extend_from_slice(extra_providers);
        let data_dirs = data_dirs
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
            clients_shipped: PathBuf::from("/usr/share/porter/clients.toml"),
            clients_user: config.join("porter/clients.toml"),
            provider_dirs,
            applications,
            tailscale_socket,
        })
    }

    /// [`Paths::provider_dirs`] with whose each is, as `resolve` laid them out: the system's,
    /// the person's, then the ones named on the command line.
    pub fn provider_layers(&self) -> Vec<(Layer, PathBuf)> {
        self.provider_dirs
            .iter()
            .enumerate()
            .map(|(at, dir)| {
                let layer = match at {
                    0 => Layer::Shipped,
                    1 => Layer::Person,
                    _ => Layer::Named,
                };
                (layer, dir.clone())
            })
            .collect()
    }

    /// The clients files the OAuth families read: the shipped one and the one Settings writes.
    pub fn client_files(&self) -> porter_families::ClientFiles {
        porter_families::ClientFiles {
            shipped: self.clients_shipped.clone(),
            own: self.clients_user.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_provider_layers_are_the_system_the_person_then_the_named() {
        let paths =
            Paths::resolve(env(&[("HOME", "/home/ada")]), &[PathBuf::from("/x")]).expect("paths");
        let layers: Vec<Layer> = paths
            .provider_layers()
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        assert_eq!(layers, [Layer::Shipped, Layer::Person, Layer::Named]);
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn tailscales_socket_is_its_own_path_unless_an_absolute_one_is_named() {
        let socket = |pairs: &[(&str, &str)]| {
            Paths::resolve(env(pairs), &[])
                .expect("paths")
                .tailscale_socket
        };
        let home = ("HOME", "/home/ada");
        assert_eq!(
            socket(&[home]),
            Path::new("/var/run/tailscale/tailscaled.sock")
        );
        assert_eq!(
            socket(&[home, (TAILSCALE_SOCKET_VAR, "/run/ts/other.sock")]),
            Path::new("/run/ts/other.sock")
        );
        // A relative path is ignored, as an XDG one is.
        assert_eq!(
            socket(&[home, (TAILSCALE_SOCKET_VAR, "ts.sock")]),
            Path::new("/var/run/tailscale/tailscaled.sock")
        );
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
    fn the_families_read_the_clients_file_settings_writes() {
        let paths = Paths::resolve(
            env(&[
                ("HOME", "/home/ada"),
                ("XDG_CONFIG_HOME", "/scratch/config"),
            ]),
            &[],
        )
        .expect("paths");
        let files = paths.client_files();
        assert_eq!(files.own, paths.clients_user);
        assert_eq!(files.own, Path::new("/scratch/config/porter/clients.toml"));
        assert_eq!(files.shipped, paths.clients_shipped);
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
