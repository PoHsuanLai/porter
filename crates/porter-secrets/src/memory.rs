//! An in-memory store: never the user's keyring, never on disk. Tests and porter-fake.

use crate::error::SecretsError;
use crate::secrets::{PutOutcome, Secrets};
use porter_core::{AccountId, Credential, SecretKey};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{Mutex, MutexGuard};

/// Credentials in a map.
#[derive(Debug, Default)]
pub struct MemorySecrets {
    entries: Mutex<HashMap<SecretKey, Credential>>,
}

impl MemorySecrets {
    fn entries(&self) -> MutexGuard<'_, HashMap<SecretKey, Credential>> {
        // A panic while the lock is held already failed the test that caused it.
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Secrets for MemorySecrets {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        self.entries().insert(key.clone(), value.clone());
        Ok(())
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        // One lock for the check and the insert: atomic.
        match self.entries().entry(key.clone()) {
            Entry::Occupied(_) => Ok(PutOutcome::AlreadyThere),
            Entry::Vacant(slot) => {
                slot.insert(value.clone());
                Ok(PutOutcome::Stored)
            }
        }
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        self.entries()
            .get(key)
            .cloned()
            .ok_or(SecretsError::Missing)
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        self.entries().remove(key);
        Ok(())
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        self.entries().retain(|key, _| key.account != *account);
        Ok(())
    }
}
