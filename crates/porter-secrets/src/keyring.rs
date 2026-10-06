//! The platform keyring store for an app that hosts porter's core itself on macOS or Windows
//! (the Apple keychain, the Windows credential manager), over keyring-core. Linux keeps its
//! secrets in the Secret Service through oo7; the keyring store there answers only once a
//! default keyring-core store is set, which is how the tests give it keyring-core's mock.
//!
//! Attribution: ported from `mail-runtime/src/secrets.rs` and `secrets/chunks.rs` in mailo
//! (MIT OR Apache-2.0, same author): the store opened once and kept, the entry-per-credential
//! layout, and the chunking for Windows' size limit.
//!
//! Blocking keyring calls run on a dedicated thread and resolve a oneshot, because this crate
//! may not reach a runtime and a blocking call on an async executor once panicked mailo.
//!
//! Each credential is one entry `{account}:{purpose}` under the service `porter`, where the
//! purpose is its serde form, as in [`attributes`]. keyring-core stores cannot be searched the
//! same way on every platform, so each account also has an index entry `{account}:index` that
//! lists its purposes, and removing an account reads it.

mod chunks;
mod thread;

use crate::attributes::{SERVICE, attributes};
use crate::error::SecretsError;
use crate::secrets::{PutOutcome, Secrets};
use chunks::{Limit, Slots};
use keyring_core::{CredentialStore, Error};
use porter_core::{AccountId, Credential, SecretKey};
use std::sync::Arc;
use thread::off_thread;

/// How much one entry holds: 2560 bytes of UTF-16 in the Credential Manager. The Keychain has no
/// limit a credential meets, so there every value is one entry.
#[cfg(windows)]
const LIMIT: Limit = Limit::Utf16Units(1280);
#[cfg(not(windows))]
const LIMIT: Limit = Limit::None;

/// The user's platform keyring.
#[derive(Debug, Default)]
pub struct KeyringSecrets;

/// Secrets over one given keyring-core store: what [`KeyringSecrets`] does once it has opened
/// the platform's, and what the tests do over keyring-core's mock.
#[derive(Debug, Clone)]
pub struct StoreSecrets {
    store: Arc<CredentialStore>,
    limit: Limit,
}

impl StoreSecrets {
    /// Files secrets in `store`, with this platform's entry size limit.
    pub fn new(store: Arc<CredentialStore>) -> Self {
        Self {
            store,
            limit: LIMIT,
        }
    }

    /// Files secrets in `store`, splitting any value over `units` UTF-16 code units, as the
    /// Windows credential manager needs. For tests of the splitting on any platform.
    pub fn chunked(store: Arc<CredentialStore>, units: usize) -> Self {
        Self {
            store,
            limit: Limit::Utf16Units(units),
        }
    }
}

/// This platform's native store, where there is one.
#[cfg(target_os = "macos")]
fn native() -> Result<Arc<CredentialStore>, SecretsError> {
    let store = apple_native_keyring_store::keychain::Store::new().map_err(refused)?;
    Ok(store)
}

#[cfg(windows)]
fn native() -> Result<Arc<CredentialStore>, SecretsError> {
    let store = windows_native_keyring_store::Store::new().map_err(refused)?;
    Ok(store)
}

/// Linux and the rest: the Secret Service is `Oo7Secrets`' job, so there is no native store.
#[cfg(not(any(target_os = "macos", windows)))]
fn native() -> Result<Arc<CredentialStore>, SecretsError> {
    Err(SecretsError::Unavailable)
}

/// The default store, or the platform's, which is then made the default. A store that could not
/// be opened is tried again on the next use, not remembered as missing.
fn resolve() -> Result<StoreSecrets, SecretsError> {
    let store = match keyring_core::get_default_store() {
        Some(store) => store,
        None => {
            let store = native()?;
            keyring_core::set_default_store(store.clone());
            store
        }
    };
    Ok(StoreSecrets::new(store))
}

fn refused(error: Error) -> SecretsError {
    match error {
        Error::NoStorageAccess(_) => SecretsError::Locked,
        Error::BadEncoding(_) | Error::BadDataFormat(..) | Error::BadStoreFormat(_) => {
            SecretsError::Unreadable
        }
        Error::NoEntry => SecretsError::Missing,
        _ => SecretsError::Unavailable,
    }
}

fn entry_name(key: &SecretKey) -> String {
    let at = attributes(key);
    format!("{}:{}", at.account, at.purpose)
}

fn index_name(account: &AccountId) -> String {
    format!("{account}:index")
}

impl Slots for StoreSecrets {
    fn read(&self, name: &str) -> Result<Option<String>, SecretsError> {
        let entry = self.store.build(SERVICE, name, None).map_err(refused)?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(refused(e)),
        }
    }

    fn write(&self, name: &str, value: &str) -> Result<(), SecretsError> {
        let entry = self.store.build(SERVICE, name, None).map_err(refused)?;
        entry.set_password(value).map_err(refused)
    }

    fn remove(&self, name: &str) -> Result<(), SecretsError> {
        let entry = self.store.build(SERVICE, name, None).map_err(refused)?;
        match entry.delete_credential() {
            // Already gone is the state we wanted.
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(e) => Err(refused(e)),
        }
    }
}

/// The purposes filed for an account; an index that does not read is as good as none.
fn purposes(store: &StoreSecrets, account: &AccountId) -> Vec<String> {
    chunks::get(store, &index_name(account))
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn write_index(
    store: &StoreSecrets,
    account: &AccountId,
    purposes: &[String],
) -> Result<(), SecretsError> {
    let name = index_name(account);
    if purposes.is_empty() {
        return chunks::forget(store, &name);
    }
    let json = serde_json::to_string(purposes).map_err(|_| SecretsError::Unreadable)?;
    chunks::put(store, &name, &json, store.limit)
}

impl StoreSecrets {
    // JSON rather than a bare string so an OAuth credential keeps its expiry and refresh token.
    fn put_now(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        let encoded = serde_json::to_string(value).map_err(|_| SecretsError::Unreadable)?;
        chunks::put(self, &entry_name(key), &encoded, self.limit)?;
        let purpose = attributes(key).purpose;
        let mut listed = purposes(self, &key.account);
        if !listed.contains(&purpose) {
            listed.push(purpose);
            write_index(self, &key.account, &listed)?;
        }
        Ok(())
    }

    /// Read, then write, in one thread job. A keyring-core store has no create-if-missing, so
    /// another process (or another `StoreSecrets` on the same store) can file the key between
    /// the two; within one process the mutex below serialises the callers of this method.
    fn put_if_absent_now(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _held = GUARD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match chunks::get(self, &entry_name(key))? {
            Some(_) => Ok(PutOutcome::AlreadyThere),
            None => self.put_now(key, value).map(|()| PutOutcome::Stored),
        }
    }

    fn get_now(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        let stored = chunks::get(self, &entry_name(key))?.ok_or(SecretsError::Missing)?;
        serde_json::from_str(&stored).map_err(|_| SecretsError::Unreadable)
    }

    fn delete_now(&self, key: &SecretKey) -> Result<(), SecretsError> {
        chunks::forget(self, &entry_name(key))?;
        let purpose = attributes(key).purpose;
        let mut listed = purposes(self, &key.account);
        if listed.contains(&purpose) {
            listed.retain(|p| *p != purpose);
            write_index(self, &key.account, &listed)?;
        }
        Ok(())
    }

    fn delete_account_now(&self, account: &AccountId) -> Result<(), SecretsError> {
        for purpose in purposes(self, account) {
            chunks::forget(self, &format!("{account}:{purpose}"))?;
        }
        chunks::forget(self, &index_name(account))
    }
}

impl Secrets for StoreSecrets {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        let (this, key, value) = (self.clone(), key.clone(), value.clone());
        off_thread(move || this.put_now(&key, &value)).await?
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        let (this, key, value) = (self.clone(), key.clone(), value.clone());
        off_thread(move || this.put_if_absent_now(&key, &value)).await?
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        let (this, key) = (self.clone(), key.clone());
        off_thread(move || this.get_now(&key)).await?
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        let (this, key) = (self.clone(), key.clone());
        off_thread(move || this.delete_now(&key)).await?
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        let (this, account) = (self.clone(), account.clone());
        off_thread(move || this.delete_account_now(&account)).await?
    }
}

impl Secrets for KeyringSecrets {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        resolve()?.put(key, value).await
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        resolve()?.get(key).await
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        resolve()?.put_if_absent(key, value).await
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        resolve()?.delete(key).await
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        resolve()?.delete_account(account).await
    }
}
