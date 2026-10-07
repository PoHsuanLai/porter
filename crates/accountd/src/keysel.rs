//! Which secret store accountd files credentials in: the Secret Service, unless a test build is
//! told otherwise.
//!
//! `ACCOUNTD_KEYS=file:<absolute path>` selects porter-secrets' `FileSecrets`, but only in a build
//! with the `test-keys` feature (off by default, never in a release or dist build). In a test
//! build any other value is a startup refusal (the same choice memoryd makes for
//! `MEMORYD_KEYS`). Without the feature the variable is ignored and, when set, one line on
//! standard error says so: there is no way to downgrade a production daemon's keys by
//! environment. The line never carries the variable's value.

use porter_core::{AccountId, Credential, SecretKey};
use porter_secrets::{Oo7Secrets, Secrets, SecretsError};
use std::path::PathBuf;

/// The environment variable the switch reads.
pub const KEYS_VAR: &str = "ACCOUNTD_KEYS";

/// Whether this build has the file key store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestKeys {
    /// Built with the `test-keys` feature.
    Built,
    /// The shipped build.
    NotBuilt,
}

impl TestKeys {
    /// What this build is.
    pub const THIS_BUILD: TestKeys = if cfg!(feature = "test-keys") {
        TestKeys::Built
    } else {
        TestKeys::NotBuilt
    };
}

/// The choice, pure over the variable's value and the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// The Secret Service (the variable is unset).
    SecretService,
    /// The Secret Service, though the variable was set: this build ignores it.
    SecretServiceIgnoring,
    /// The key file at this absolute path.
    File(PathBuf),
}

/// Why accountd refuses to start over the keys it was told to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeysRefusal {
    /// In a test build the variable is set but is not `file:<path>`.
    NotFilePath,
    /// The path after `file:` is not absolute.
    NotAbsolute,
    /// A file store was chosen in a build without the feature (unreachable through [`select`]).
    NotBuilt,
    /// The key file cannot be used.
    #[cfg(feature = "test-keys")]
    File(porter_secrets::FileSecretsError),
}

impl std::fmt::Display for KeysRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFilePath => write!(f, "{KEYS_VAR} is not file:<absolute path>"),
            Self::NotAbsolute => write!(f, "{KEYS_VAR}: the path after file: is not absolute"),
            Self::NotBuilt => f.write_str("a file key store needs the test-keys feature"),
            #[cfg(feature = "test-keys")]
            Self::File(why) => write!(f, "{KEYS_VAR}: {why}"),
        }
    }
}

impl std::error::Error for KeysRefusal {}

/// Chooses the key store.
pub fn select(var: Option<&str>, build: TestKeys) -> Result<Selection, KeysRefusal> {
    match (var, build) {
        (None, _) => Ok(Selection::SecretService),
        (Some(_), TestKeys::NotBuilt) => Ok(Selection::SecretServiceIgnoring),
        (Some(value), TestKeys::Built) => {
            let path = value
                .strip_prefix("file:")
                .filter(|path| !path.is_empty())
                .ok_or(KeysRefusal::NotFilePath)?;
            let path = PathBuf::from(path);
            path.is_absolute()
                .then_some(Selection::File(path))
                .ok_or(KeysRefusal::NotAbsolute)
        }
    }
}

/// The secret store accountd runs with.
#[derive(Debug, Clone)]
pub enum AnyKeys {
    /// The session's Secret Service.
    Oo7(Oo7Secrets),
    /// A key file (test builds only).
    #[cfg(feature = "test-keys")]
    File(porter_secrets::FileSecrets),
}

/// The store, and the line accountd says on standard error about which one it is.
#[derive(Debug, Clone)]
pub struct Chosen {
    /// The store.
    pub keys: AnyKeys,
    /// The one line about the choice: never a credential, never the variable's value.
    pub said: String,
}

impl Chosen {
    /// Opens the store `selection` names.
    pub fn open(selection: Selection) -> Result<Self, KeysRefusal> {
        match selection {
            Selection::SecretService => Ok(Self {
                keys: AnyKeys::Oo7(Oo7Secrets),
                said: "secret store: the Secret Service".to_owned(),
            }),
            Selection::SecretServiceIgnoring => Ok(Self {
                keys: AnyKeys::Oo7(Oo7Secrets),
                said: format!(
                    "{KEYS_VAR} is set but this build has no test-keys feature; ignoring it and using the Secret Service"
                ),
            }),
            #[cfg(feature = "test-keys")]
            Selection::File(path) => {
                let store = porter_secrets::FileSecrets::open(&path).map_err(KeysRefusal::File)?;
                Ok(Self {
                    said: format!(
                        "TEST BUILD: credentials are in the file {}, not the Secret Service",
                        path.display()
                    ),
                    keys: AnyKeys::File(store),
                })
            }
            #[cfg(not(feature = "test-keys"))]
            Selection::File(_) => Err(KeysRefusal::NotBuilt),
        }
    }

    /// The choice for this process: its environment variable and its build.
    pub fn from_env() -> Result<Self, KeysRefusal> {
        let var = std::env::var(KEYS_VAR).ok();
        Self::open(select(var.as_deref(), TestKeys::THIS_BUILD)?)
    }
}

impl Secrets for AnyKeys {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        match self {
            Self::Oo7(keys) => keys.put(key, value).await,
            #[cfg(feature = "test-keys")]
            Self::File(keys) => keys.put(key, value).await,
        }
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<porter_secrets::PutOutcome, SecretsError> {
        match self {
            Self::Oo7(keys) => keys.put_if_absent(key, value).await,
            #[cfg(feature = "test-keys")]
            Self::File(keys) => keys.put_if_absent(key, value).await,
        }
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        match self {
            Self::Oo7(keys) => keys.get(key).await,
            #[cfg(feature = "test-keys")]
            Self::File(keys) => keys.get(key).await,
        }
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        match self {
            Self::Oo7(keys) => keys.delete(key).await,
            #[cfg(feature = "test-keys")]
            Self::File(keys) => keys.delete(key).await,
        }
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        match self {
            Self::Oo7(keys) => keys.delete_account(account).await,
            #[cfg(feature = "test-keys")]
            Self::File(keys) => keys.delete_account(account).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_is_the_secret_service_in_every_build() {
        for build in [TestKeys::Built, TestKeys::NotBuilt] {
            assert_eq!(select(None, build), Ok(Selection::SecretService));
        }
    }

    #[test]
    fn a_shipped_build_ignores_the_variable_whatever_it_says_and_never_repeats_it() {
        for value in ["file:/x/keys", "garbage", "", "file:relative"] {
            assert_eq!(
                select(Some(value), TestKeys::NotBuilt),
                Ok(Selection::SecretServiceIgnoring),
                "{value}"
            );
        }
        let said = Chosen::open(Selection::SecretServiceIgnoring)
            .expect("ignored, not refused")
            .said;
        assert!(
            said.contains("ignoring") && said.contains(KEYS_VAR),
            "{said}"
        );
        assert!(!said.contains("/x/keys"));
    }

    #[test]
    fn a_test_build_reads_absolute_file_paths_and_refuses_everything_else() {
        let cases = [
            (
                "file:/x/keys",
                Ok(Selection::File(PathBuf::from("/x/keys"))),
            ),
            ("file:", Err(KeysRefusal::NotFilePath)),
            ("/x/keys", Err(KeysRefusal::NotFilePath)),
            ("", Err(KeysRefusal::NotFilePath)),
            ("memory", Err(KeysRefusal::NotFilePath)),
            ("File:/x/keys", Err(KeysRefusal::NotFilePath)),
            ("file:keys", Err(KeysRefusal::NotAbsolute)),
            ("file:./keys", Err(KeysRefusal::NotAbsolute)),
        ];
        for (value, want) in cases {
            assert_eq!(select(Some(value), TestKeys::Built), want, "{value}");
        }
    }

    #[test]
    fn unset_opens_the_secret_service_without_touching_it() {
        let chosen = Chosen::open(Selection::SecretService).expect("chosen");
        assert!(matches!(chosen.keys, AnyKeys::Oo7(_)));
        assert!(chosen.said.contains("Secret Service"));
    }

    #[test]
    fn this_build_is_what_its_features_say() {
        let chosen = select(Some("file:/x/keys"), TestKeys::THIS_BUILD);
        if cfg!(feature = "test-keys") {
            assert_eq!(chosen, Ok(Selection::File(PathBuf::from("/x/keys"))));
        } else {
            assert_eq!(chosen, Ok(Selection::SecretServiceIgnoring));
        }
    }

    #[cfg(feature = "test-keys")]
    #[test]
    fn a_key_file_open_to_others_is_a_typed_refusal() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("accountd-keysel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("keys.json");
        let _ = std::fs::remove_file(&path);
        let chosen = Chosen::open(Selection::File(path.clone())).expect("created");
        assert!(chosen.said.starts_with("TEST BUILD"), "{}", chosen.said);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let refused = Chosen::open(Selection::File(path)).expect_err("loose mode");
        assert!(matches!(refused, KeysRefusal::File(_)), "{refused}");
        assert!(refused.to_string().contains("0600"), "{refused}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
