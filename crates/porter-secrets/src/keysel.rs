//! Which secret store a daemon files credentials in: the Secret Service in its own namespace,
//! unless a test build is told otherwise.
//!
//! A daemon names its own environment variable (`ACCOUNTD_KEYS`, `INFERD_KEYS`, `SYNCD_KEYS`)
//! and passes its value to [`select`]; this library reads no environment. `file:<absolute path>`
//! selects [`FileSecrets`](crate::FileSecrets), but only in a build with the `test-keys` feature
//! (off by default, never in a release or dist build). In a test build any other value is a
//! startup refusal. Without the feature the value is ignored and, when set, one line says so:
//! there is no way to downgrade a production daemon's keys by environment. A line never carries
//! the variable's value.
//!
//! The module is built with the `oo7` feature (the Secret Service store); the key file arm
//! needs `test-keys` on a unix target as well.

use crate::{Namespace, Oo7NamedSecrets, Oo7Secrets, PutOutcome, Secrets, SecretsError};
use porter_core::{AccountId, Credential, SecretKey};
use std::path::PathBuf;

/// Whether this build has the file key store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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
#[non_exhaustive]
pub enum Selection {
    /// The Secret Service (the variable is unset).
    SecretService,
    /// The Secret Service, though the variable was set: this build ignores it.
    SecretServiceIgnoring,
    /// The key file at this absolute path.
    File(PathBuf),
}

/// Why a daemon refuses to start over the keys it was told to use.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeysRefusal {
    /// In a test build the variable is set but is not `file:<path>`.
    NotFilePath,
    /// The path after `file:` is not absolute.
    NotAbsolute,
    /// A file store was chosen in a build without the feature (unreachable through [`select`]).
    NotBuilt,
    /// The key file cannot be used.
    #[cfg(all(feature = "test-keys", unix))]
    File(crate::FileSecretsError),
}

impl std::fmt::Display for KeysRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFilePath => f.write_str("the key store setting is not file:<absolute path>"),
            Self::NotAbsolute => {
                f.write_str("the key store setting: the path after file: is not absolute")
            }
            Self::NotBuilt => f.write_str("a file key store needs the test-keys feature"),
            #[cfg(all(feature = "test-keys", unix))]
            Self::File(why) => write!(f, "the key store setting: {why}"),
        }
    }
}

impl std::error::Error for KeysRefusal {}

/// Chooses the key store for this build from the value of the daemon's variable.
pub fn select(var_value: Option<&str>) -> Result<Selection, KeysRefusal> {
    select_for(var_value, TestKeys::THIS_BUILD)
}

/// Chooses the key store, pure over the variable's value and `build`.
pub fn select_for(var_value: Option<&str>, build: TestKeys) -> Result<Selection, KeysRefusal> {
    match (var_value, build) {
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

/// The secret store a daemon runs with.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AnyKeys {
    /// The session's Secret Service, in the daemon's namespace.
    Oo7(Oo7NamedSecrets),
    /// A key file (test builds only).
    #[cfg(all(feature = "test-keys", unix))]
    File(crate::FileSecrets),
}

/// The store, and the line a daemon says on standard error about which one it is.
#[derive(Debug, Clone)]
pub struct Chosen {
    /// The store.
    pub keys: AnyKeys,
    /// The one line about the choice: never a credential, never the variable's value.
    pub said: String,
}

impl Chosen {
    /// Opens the store `selection` names, filing under `namespace`. `said_prefix` starts the
    /// line about the choice: the daemon's name or its variable's (`"inferd"`, `"INFERD_KEYS"`).
    pub fn open(
        selection: Selection,
        namespace: Namespace,
        said_prefix: &str,
    ) -> Result<Self, KeysRefusal> {
        match selection {
            Selection::SecretService => Ok(Self {
                keys: AnyKeys::Oo7(Oo7Secrets::in_namespace(namespace)),
                said: format!("{said_prefix}: secret store: the Secret Service"),
            }),
            Selection::SecretServiceIgnoring => Ok(Self {
                keys: AnyKeys::Oo7(Oo7Secrets::in_namespace(namespace)),
                said: format!(
                    "{said_prefix}: the key store setting is set but this build has no test-keys feature; ignoring it and using the Secret Service"
                ),
            }),
            #[cfg(all(feature = "test-keys", unix))]
            Selection::File(path) => {
                let store =
                    crate::FileSecrets::open_in(&path, namespace).map_err(KeysRefusal::File)?;
                Ok(Self {
                    said: format!(
                        "{said_prefix}: TEST BUILD: credentials are in the file {}, not the Secret Service",
                        path.display()
                    ),
                    keys: AnyKeys::File(store),
                })
            }
            #[cfg(not(all(feature = "test-keys", unix)))]
            Selection::File(_) => Err(KeysRefusal::NotBuilt),
        }
    }
}

impl Secrets for AnyKeys {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        match self {
            Self::Oo7(keys) => keys.put(key, value).await,
            #[cfg(all(feature = "test-keys", unix))]
            Self::File(keys) => keys.put(key, value).await,
        }
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        match self {
            Self::Oo7(keys) => keys.put_if_absent(key, value).await,
            #[cfg(all(feature = "test-keys", unix))]
            Self::File(keys) => keys.put_if_absent(key, value).await,
        }
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        match self {
            Self::Oo7(keys) => keys.get(key).await,
            #[cfg(all(feature = "test-keys", unix))]
            Self::File(keys) => keys.get(key).await,
        }
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        match self {
            Self::Oo7(keys) => keys.delete(key).await,
            #[cfg(all(feature = "test-keys", unix))]
            Self::File(keys) => keys.delete(key).await,
        }
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        match self {
            Self::Oo7(keys) => keys.delete_account(account).await,
            #[cfg(all(feature = "test-keys", unix))]
            Self::File(keys) => keys.delete_account(account).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namespace() -> Namespace {
        Namespace::parse("porter.test").expect("a namespace")
    }

    #[test]
    fn unset_is_the_secret_service_in_every_build() {
        for build in [TestKeys::Built, TestKeys::NotBuilt] {
            assert_eq!(select_for(None, build), Ok(Selection::SecretService));
        }
    }

    #[test]
    fn a_shipped_build_ignores_the_variable_whatever_it_says_and_never_repeats_it() {
        for value in ["file:/x/keys", "garbage", "", "file:relative"] {
            assert_eq!(
                select_for(Some(value), TestKeys::NotBuilt),
                Ok(Selection::SecretServiceIgnoring),
                "{value}"
            );
        }
        let said = Chosen::open(Selection::SecretServiceIgnoring, namespace(), "TEST_KEYS")
            .expect("ignored, not refused")
            .said;
        assert!(
            said.contains("ignoring") && said.contains("TEST_KEYS"),
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
            assert_eq!(select_for(Some(value), TestKeys::Built), want, "{value}");
        }
    }

    #[test]
    fn unset_opens_the_secret_service_in_the_daemons_namespace_without_touching_it() {
        let chosen = Chosen::open(Selection::SecretService, namespace(), "inferd").expect("chosen");
        let AnyKeys::Oo7(keys) = &chosen.keys else {
            panic!("the Secret Service");
        };
        assert_eq!(keys.namespace(), &namespace());
        assert!(chosen.said.starts_with("inferd:"), "{}", chosen.said);
        assert!(chosen.said.contains("Secret Service"));
    }

    #[test]
    fn this_build_is_what_its_features_say() {
        let chosen = select(Some("file:/x/keys"));
        if cfg!(feature = "test-keys") {
            assert_eq!(chosen, Ok(Selection::File(PathBuf::from("/x/keys"))));
        } else {
            assert_eq!(chosen, Ok(Selection::SecretServiceIgnoring));
        }
    }

    #[cfg(all(feature = "test-keys", unix))]
    #[test]
    fn a_key_file_open_to_others_is_a_typed_refusal() {
        use std::os::unix::fs::PermissionsExt;
        let dir = test_kit::Scratch::new("keysel");
        let path = dir.join("keys.json");
        let chosen =
            Chosen::open(Selection::File(path.clone()), namespace(), "syncd").expect("created");
        assert!(
            chosen.said.starts_with("syncd: TEST BUILD"),
            "{}",
            chosen.said
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let refused =
            Chosen::open(Selection::File(path), namespace(), "syncd").expect_err("loose mode");
        assert!(matches!(refused, KeysRefusal::File(_)), "{refused}");
        assert!(refused.to_string().contains("0600"), "{refused}");
    }
}
