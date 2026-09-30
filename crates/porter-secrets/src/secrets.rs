//! The store seam: the Secret Service (oo7), a platform keyring, or the in-memory fake.

use crate::error::SecretsError;
use porter_core::{AccountId, Credential, SecretKey};
use std::future::Future;

/// A store of credentials, filed by [`SecretKey`].
pub trait Secrets: Send + Sync {
    /// Files `value` under `key`, replacing what was there.
    fn put(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> impl Future<Output = Result<(), SecretsError>> + Send;

    /// The credential filed under `key`.
    fn get(&self, key: &SecretKey)
    -> impl Future<Output = Result<Credential, SecretsError>> + Send;

    /// Removes the credential under `key`; removing a missing one succeeds.
    fn delete(&self, key: &SecretKey) -> impl Future<Output = Result<(), SecretsError>> + Send;

    /// Removes every credential of `account`, in one step, when the account is removed.
    fn delete_account(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<(), SecretsError>> + Send;
}
