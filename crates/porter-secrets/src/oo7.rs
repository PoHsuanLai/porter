//! The Secret Service store through oo7 (Linux). oo7 picks its own backend: its file backend,
//! keyed by the Secret portal, inside a sandbox, and the user's default Secret Service
//! collection everywhere else. Where neither answers the store is [`SecretsError::Unavailable`].

use crate::attributes::{SERVICE, attributes};
use crate::error::SecretsError;
use crate::secrets::{PutOutcome, Secrets};
use porter_core::{AccountId, Credential, SecretKey};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The user's Secret Service collection, opened on first use and kept.
#[derive(Debug, Default)]
pub struct Oo7Secrets;

/// Secrets over one given oo7 keyring: what [`Oo7Secrets`] does once it has opened its own, and
/// what the tests do over oo7's file backend in a scratch directory.
#[derive(Debug, Clone)]
pub struct Oo7KeyringSecrets {
    keyring: Arc<oo7::Keyring>,
}

impl Oo7KeyringSecrets {
    /// Files secrets in `keyring`.
    pub fn new(keyring: oo7::Keyring) -> Self {
        Self {
            keyring: Arc::new(keyring),
        }
    }
}

/// The opened keyring. A keyring that could not be opened is tried again on the next call: a
/// Secret Service that was not up at the first call (a session whose keyring daemon starts
/// late) is up for the second.
static OPENED: Mutex<Option<Oo7KeyringSecrets>> = Mutex::new(None);

async fn ambient() -> Result<Oo7KeyringSecrets, SecretsError> {
    let held = OPENED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if let Some(store) = held {
        return Ok(store);
    }
    let store = Oo7KeyringSecrets::new(oo7::Keyring::new().await.map_err(refused)?);
    *OPENED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(store.clone());
    Ok(store)
}

fn refused(error: oo7::Error) -> SecretsError {
    use oo7::dbus::Error as Bus;
    use oo7::file::Error as File;
    match error {
        oo7::Error::File(File::Locked | File::IncorrectSecret)
        | oo7::Error::DBus(Bus::Dismissed) => SecretsError::Locked,
        oo7::Error::File(File::Portal(_)) | oo7::Error::File(_) | oo7::Error::DBus(_) => {
            SecretsError::Unavailable
        }
    }
}

fn search(key: &SecretKey) -> HashMap<&'static str, String> {
    let at = attributes(key);
    HashMap::from([
        ("service", at.service.to_owned()),
        ("account", at.account),
        ("purpose", at.purpose),
    ])
}

impl Secrets for Oo7KeyringSecrets {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        let encoded = serde_json::to_vec(value).map_err(|_| SecretsError::Unreadable)?;
        let label = format!("porter {}", key.account);
        self.keyring
            .create_item(&label, &search(key), oo7::Secret::blob(encoded), true)
            .await
            .map_err(refused)
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        let encoded = serde_json::to_vec(value).map_err(|_| SecretsError::Unreadable)?;
        let label = format!("porter {}", key.account);
        let at = search(key);
        match &*self.keyring {
            // The file backend: the keyring's own write lock is held across the look and the
            // create, which excludes every other user of this open keyring. The inner calls
            // take only the inner keyring's lock, never this one.
            oo7::Keyring::File(shared) => {
                let guard = shared.write().await;
                let Some(oo7::file::Keyring::Unlocked(file)) = guard.as_ref() else {
                    return Err(SecretsError::Locked);
                };
                match file.lookup_item(&at).await.map_err(|e| refused(e.into()))? {
                    Some(_) => Ok(PutOutcome::AlreadyThere),
                    None => file
                        .create_item(&label, &at, oo7::Secret::blob(encoded), false)
                        .await
                        .map(|_| PutOutcome::Stored)
                        .map_err(|e| refused(e.into())),
                }
            }
            // The Secret Service has no create-if-missing: read, then create, with a window.
            oo7::Keyring::DBus(_) => {
                let found = self.keyring.search_items(&at).await.map_err(refused)?;
                match found.is_empty() {
                    false => Ok(PutOutcome::AlreadyThere),
                    true => self
                        .keyring
                        .create_item(&label, &at, oo7::Secret::blob(encoded), false)
                        .await
                        .map(|()| PutOutcome::Stored)
                        .map_err(refused),
                }
            }
        }
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        let items = self
            .keyring
            .search_items(&search(key))
            .await
            .map_err(refused)?;
        let item = items.first().ok_or(SecretsError::Missing)?;
        let secret = item.secret().await.map_err(refused)?;
        serde_json::from_slice(secret.as_bytes()).map_err(|_| SecretsError::Unreadable)
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        self.keyring.delete(&search(key)).await.map_err(refused)
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        let account = HashMap::from([
            ("service", SERVICE.to_owned()),
            ("account", account.to_string()),
        ]);
        self.keyring.delete(&account).await.map_err(refused)
    }
}

impl Secrets for Oo7Secrets {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        ambient().await?.put(key, value).await
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        ambient().await?.get(key).await
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        ambient().await?.put_if_absent(key, value).await
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        ambient().await?.delete(key).await
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        ambient().await?.delete_account(account).await
    }
}
