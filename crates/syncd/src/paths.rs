//! Where syncd reads and writes, resolved from the environment (PLAN §2.3): the one place the
//! daemon names a path, so a test can give it scratch directories. An account is named on disk
//! by its object-path segment (`porter_ids::object_segment`), the form the account's
//! directories are named in, so a wipe needs no map back to the id.

pub use porter_daemon::xdg::PathError;
use porter_daemon::xdg::Xdg;
use std::path::PathBuf;
use std::time::Duration;

/// Every path the daemon uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `$XDG_STATE_HOME/porter/sync`: one directory per account, one journal per dataset.
    pub journals: PathBuf,
    /// `$XDG_DATA_HOME/porter/vdir`: the PIM mirrors, one directory per account (W6e).
    pub mirrors: PathBuf,
    /// The system caller table, `/etc/porter/callers.toml` (another file in a test build:
    /// [`porter_callers::SystemCallers`]).
    pub callers_system: PathBuf,
    /// The user's caller table, `$XDG_CONFIG_HOME/porter/callers.toml`; its rows win.
    pub callers_user: PathBuf,
    /// `~/Cloud`: where each drive the person shows in Files is a folder (`~/Cloud/OneDrive`).
    pub cloud: PathBuf,
    /// `$XDG_CACHE_HOME/porter/drives`: each drive's downloaded files, one directory per account.
    pub drive_cache: PathBuf,
    /// `$XDG_CONFIG_HOME/porter/syncd/syncd.toml`: syncd's own settings (`storage.cache_limit`),
    /// in a folder of its own so that syncd's unit may write it and nothing else of porter's
    /// configuration (accountd's caller table sits in `porter/`). The file kept at
    /// `porter/syncd.toml` before is read while this one is absent.
    pub settings: PathBuf,
}

impl Paths {
    /// The paths for the environment `var` reads.
    pub fn resolve(var: impl Fn(&str) -> Option<String>) -> Result<Self, PathError> {
        let xdg = Xdg::new(var);
        let state = xdg.dir("XDG_STATE_HOME", ".local/state")?;
        let config = xdg.dir("XDG_CONFIG_HOME", ".config")?;
        let data = xdg.dir("XDG_DATA_HOME", ".local/share")?;
        let cache = xdg.dir("XDG_CACHE_HOME", ".cache")?;
        let home = xdg.home().ok_or(PathError::NoHome)?.to_path_buf();
        Ok(Self {
            journals: state.join("porter/sync"),
            mirrors: data.join("porter/vdir"),
            callers_system: PathBuf::from(porter_callers::SYSTEM_CALLERS),
            callers_user: config.join("porter/callers.toml"),
            cloud: home.join("Cloud"),
            drive_cache: cache.join("porter/drives"),
            settings: config.join("porter/syncd/syncd.toml"),
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

    /// The folder whose files syncd uploads to the account's Google Photos,
    /// `$XDG_DATA_HOME/porter/photos/<account>/upload`: inside the Photos library directory, so
    /// `AccountRemoved` wipes it with the rest.
    pub fn photos_upload_dir(&self, account: &AccountDir) -> PathBuf {
        self.photos_dir(account).join("upload")
    }

    /// Where the Google Photos picker puts what the person picked,
    /// `$XDG_DATA_HOME/porter/photos/<account>/picked`; one folder per session below it.
    pub fn photos_picked_dir(&self, account: &AccountDir) -> PathBuf {
        self.photos_dir(account).join("picked")
    }

    /// The ledger of what was uploaded to Google Photos (the album and every content hash sent),
    /// beside the account's journals so it goes with them.
    pub fn photos_ledger(&self, account: &AccountDir) -> PathBuf {
        self.journals
            .join(account.as_str())
            .join("google_photos_upload.ledger.json")
    }

    /// The downloaded files of one account's drive, `$XDG_CACHE_HOME/porter/drives/<account>`.
    pub fn drive_cache_dir(&self, account: &AccountDir) -> PathBuf {
        self.drive_cache.join(account.as_str())
    }

    /// Everything of one account that syncd keeps: its journals and its mirrors.
    pub fn account_dirs(&self, account: &AccountDir) -> [PathBuf; 2] {
        [
            self.journals.join(account.as_str()),
            self.mirrors.join(account.as_str()),
        ]
    }
}

/// Where syncd keeps its own accounts, apart from the datasets' journals and mirrors: its
/// registry, audit file, provider files and the sign-in keys the families read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct KeepPaths {
    /// The directory of syncd's `registry.json`: `$XDG_STATE_HOME/quire/syncd/keys`.
    pub registry_dir: PathBuf,
    /// `$XDG_STATE_HOME/quire/syncd/audit.jsonl`.
    pub audit: PathBuf,
    /// Provider file directories, later ones winning: the system's (under the first
    /// data directory that has `porter/providers`), then the person's.
    pub provider_dirs: Vec<PathBuf>,
    /// The shipped clients: `porter/clients.toml` under the first data directory that has it.
    pub clients_shipped: PathBuf,
    /// The person's clients, `$XDG_CONFIG_HOME/porter/clients.toml` (Settings writes it).
    pub clients_user: PathBuf,
}

impl KeepPaths {
    /// The paths for the environment `var` reads.
    pub fn resolve(var: impl Fn(&str) -> Option<String>) -> Result<Self, PathError> {
        let xdg = Xdg::new(var);
        let state = xdg.dir("XDG_STATE_HOME", ".local/state")?;
        let config = xdg.dir("XDG_CONFIG_HOME", ".config")?;
        let data = xdg.dir("XDG_DATA_HOME", ".local/share")?;
        Ok(Self {
            registry_dir: state.join("quire/syncd/keys"),
            audit: state.join("quire/syncd/audit.jsonl"),
            provider_dirs: vec![
                xdg.shipped("porter/providers"),
                data.join("porter/providers"),
            ],
            clients_shipped: xdg.shipped("porter/clients.toml"),
            clients_user: config.join("porter/clients.toml"),
        })
    }

    /// Everything under `root` (a scratch directory), with no provider files and no clients.
    pub fn under(root: &std::path::Path) -> Self {
        Self {
            registry_dir: root.join("keys"),
            audit: root.join("audit.jsonl"),
            provider_dirs: Vec::new(),
            clients_shipped: root.join("no-clients.toml"),
            clients_user: root.join("no-clients-own.toml"),
        }
    }

    /// [`KeepPaths::provider_dirs`] with whose each is: the system's, the person's, then any
    /// others (trusted as whoever started the daemon).
    pub fn provider_layers(&self) -> Vec<(porter_families::set::Layer, PathBuf)> {
        use porter_families::set::Layer;
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

    /// The clients files the sign-in families read.
    pub fn client_files(&self) -> porter_family_oauth::ClientFiles {
        porter_family_oauth::ClientFiles {
            shipped: self.clients_shipped.clone(),
            own: self.clients_user.clone(),
        }
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

/// Whether this binary honours `SYNCD_PROC_ROOT`: only with the `test-proc-root` feature. A
/// release build ignores the variable.
pub const HONOURS_PROC_ROOT: bool = cfg!(feature = "test-proc-root");

/// How often the supervisors read grants again (`SYNCD_RESCAN_S`, whole seconds, at least 1),
/// for a test build only, so a scenario need not wait the default ten minutes for a grant it
/// just made. A release build ignores the variable; so does a value that is not a number of
/// seconds. The default is `default`, and no build has a shorter one.
pub fn rescan(build: Build, value: Option<String>, default: Duration) -> Duration {
    match build {
        Build::Test => value
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds >= 1)
            .map_or(default, Duration::from_secs),
        Build::Release => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_rescan_variable_is_read_by_a_test_build_only_and_never_shortens_the_default() {
        let default = Duration::from_secs(600);
        let cases = [
            (Build::Test, Some("2"), Duration::from_secs(2)),
            (Build::Test, Some(" 5 "), Duration::from_secs(5)),
            (Build::Test, Some("0"), default),
            (Build::Test, Some("soon"), default),
            (Build::Test, Some("-3"), default),
            (Build::Test, None, default),
            (Build::Release, Some("2"), default),
            (Build::Release, None, default),
        ];
        for (build, value, want) in cases {
            assert_eq!(
                rescan(build, value.map(str::to_owned), default),
                want,
                "{build:?} {value:?}"
            );
        }
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
    fn syncds_own_accounts_live_under_quire_syncd_and_the_provider_layers_run_system_person_named()
    {
        let keep = KeepPaths::resolve(env(&[("HOME", "/home/ada")])).expect("paths");
        assert_eq!(
            keep.registry_dir,
            Path::new("/home/ada/.local/state/quire/syncd/keys")
        );
        assert_eq!(
            keep.audit,
            Path::new("/home/ada/.local/state/quire/syncd/audit.jsonl")
        );
        let mut more = keep.clone();
        more.provider_dirs.push(PathBuf::from("/x"));
        let layers: Vec<_> = more.provider_layers().into_iter().map(|(l, _)| l).collect();
        use porter_families::set::Layer;
        assert_eq!(layers, [Layer::Shipped, Layer::Person, Layer::Named]);
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
