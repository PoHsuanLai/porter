//! Where syncd reads and writes, resolved from the environment (PLAN §2.3): the one place the
//! daemon names a path, so a test can give it scratch directories. An account is named on disk
//! by its object-path segment (`porter_core::object_segment`), the form `AccountRemoved`
//! carries, so a wipe needs no map back to the id.

use std::path::PathBuf;

/// Every path the daemon uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `$XDG_STATE_HOME/porter/sync`: one directory per account, one journal per dataset.
    pub journals: PathBuf,
    /// `$XDG_DATA_HOME/porter/vdir`: the PIM mirrors, one directory per account (W6e).
    pub mirrors: PathBuf,
    /// The system caller table, `/etc/porter/callers.toml`.
    pub callers_system: PathBuf,
    /// The user's caller table, `$XDG_CONFIG_HOME/porter/callers.toml`; its rows win.
    pub callers_user: PathBuf,
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
    /// The paths for the environment `var` reads.
    pub fn resolve(var: impl Fn(&str) -> Option<String>) -> Result<Self, PathError> {
        let home = absolute(var("HOME"));
        let under = |name: &str, tail: &str| match (absolute(var(name)), &home) {
            (Some(dir), _) => Ok(dir),
            (None, Some(home)) => Ok(home.join(tail)),
            (None, None) => Err(PathError::NoHome),
        };
        let state = under("XDG_STATE_HOME", ".local/state")?;
        let config = under("XDG_CONFIG_HOME", ".config")?;
        let data = under("XDG_DATA_HOME", ".local/share")?;
        Ok(Self {
            journals: state.join("porter/sync"),
            mirrors: data.join("porter/vdir"),
            callers_system: PathBuf::from("/etc/porter/callers.toml"),
            callers_user: config.join("porter/callers.toml"),
        })
    }

    /// The journal file of one dataset of one account.
    pub fn journal(&self, account: &AccountDir, dataset: &str) -> PathBuf {
        self.journals
            .join(account.as_str())
            .join(format!("{dataset}.sqlite"))
    }

    /// The Photos library of one account, `$XDG_DATA_HOME/porter/photos/<account>` (W6f): beside
    /// the PIM mirrors, so `AccountRemoved` wipes it with them.
    pub fn photos_dir(&self, account: &AccountDir) -> PathBuf {
        self.mirrors
            .parent()
            .unwrap_or(&self.mirrors)
            .join("photos")
            .join(account.as_str())
    }

    /// The app folder mirror of one account, `$XDG_DATA_HOME/porter/storage/<account>`: beside
    /// the PIM mirrors, so `AccountRemoved` wipes it with them.
    pub fn storage_dir(&self, account: &AccountDir) -> PathBuf {
        self.mirrors
            .parent()
            .unwrap_or(&self.mirrors)
            .join("storage")
            .join(account.as_str())
    }

    /// Everything of one account that syncd keeps: its journals and its mirrors.
    pub fn account_dirs(&self, account: &AccountDir) -> [PathBuf; 2] {
        [
            self.journals.join(account.as_str()),
            self.mirrors.join(account.as_str()),
        ]
    }
}

/// An account as a directory name: the `[A-Za-z0-9_]+` segment of its object path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountDir(String);

impl AccountDir {
    /// The segment, if it is one: nothing else may become a path component.
    pub fn parse(segment: &str) -> Option<Self> {
        let ok = !segment.is_empty()
            && segment.len() <= 64
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_');
        ok.then(|| Self(segment.to_owned()))
    }

    /// The account the object path `/org/quire/Accounts1/account/<segment>` names.
    pub fn of_object_path(path: &str) -> Option<Self> {
        let prefix = format!("{}/account/", porter_dbus::ACCOUNTS_PATH);
        Self::parse(path.strip_prefix(&prefix)?)
    }

    /// The directory name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccountDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
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

/// The directory callers are read from in place of `/proc` (`SYNCD_PROC_ROOT`), for a test
/// build only. A release build ignores the variable.
pub fn proc_root(build: Build, value: Option<String>) -> Option<PathBuf> {
    match build {
        Build::Test => value.filter(|v| !v.is_empty()).map(PathBuf::from),
        Build::Release => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
        let paths = Paths::resolve(env(&[("HOME", "/home/ada")])).expect("paths");
        assert_eq!(
            paths.journals,
            Path::new("/home/ada/.local/state/porter/sync")
        );
        assert_eq!(
            paths.mirrors,
            Path::new("/home/ada/.local/share/porter/vdir")
        );
        assert_eq!(
            paths.callers_user,
            Path::new("/home/ada/.config/porter/callers.toml")
        );
    }

    #[test]
    fn xdg_variables_win_and_relative_ones_are_ignored() {
        let paths = Paths::resolve(env(&[
            ("HOME", "/home/ada"),
            ("XDG_STATE_HOME", "/scratch/state"),
            ("XDG_DATA_HOME", "relative/data"),
        ]))
        .expect("paths");
        assert_eq!(paths.journals, Path::new("/scratch/state/porter/sync"));
        assert_eq!(
            paths.mirrors,
            Path::new("/home/ada/.local/share/porter/vdir")
        );
        assert_eq!(Paths::resolve(env(&[])), Err(PathError::NoHome));
    }

    #[test]
    fn a_photos_library_lives_beside_the_vdir_under_its_account() {
        let paths = Paths::resolve(env(&[("HOME", "/h")])).expect("paths");
        let account = AccountDir::parse("a1").expect("segment");
        assert_eq!(
            paths.photos_dir(&account),
            Path::new("/h/.local/share/porter/photos/a1")
        );
    }

    #[test]
    fn a_journal_lives_under_its_account_and_a_wipe_names_journals_and_mirrors() {
        let paths = Paths::resolve(env(&[("HOME", "/h")])).expect("paths");
        let account = AccountDir::parse("67e55044_10b1").expect("segment");
        assert_eq!(
            paths.journal(&account, "files"),
            Path::new("/h/.local/state/porter/sync/67e55044_10b1/files.sqlite")
        );
        let [journals, mirrors] = paths.account_dirs(&account);
        assert_eq!(
            journals,
            Path::new("/h/.local/state/porter/sync/67e55044_10b1")
        );
        assert_eq!(
            mirrors,
            Path::new("/h/.local/share/porter/vdir/67e55044_10b1")
        );
    }

    #[test]
    fn only_a_plain_segment_becomes_a_directory_name() {
        const CASES: &[(&str, bool)] = &[
            ("abc_123", true),
            ("", false),
            ("..", false),
            ("a/b", false),
            ("a-b", false),
            ("a.b", false),
        ];
        for (text, ok) in CASES {
            assert_eq!(AccountDir::parse(text).is_some(), *ok, "{text:?}");
        }
        let path = "/org/quire/Accounts1/account/abc_1";
        assert_eq!(
            AccountDir::of_object_path(path).map(|a| a.to_string()),
            Some("abc_1".into())
        );
        assert_eq!(
            AccountDir::of_object_path("/org/quire/Accounts1/account/../x"),
            None
        );
        assert_eq!(AccountDir::of_object_path("/other/abc"), None);
    }

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
