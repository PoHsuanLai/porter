//! The store seam: the Secret Service (oo7), a platform keyring, or the in-memory fake.

use crate::error::SecretsError;
use porter_core::AccountId;
use porter_core::{Credential, SecretKey};
use std::future::Future;

/// A store of credentials, filed by [`SecretKey`].
pub trait Secrets: Send + Sync {
    /// Files `value` under `key`, replacing what was there.
    fn put(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> impl Future<Output = Result<(), SecretsError>> + Send;

    /// Files `value` under `key` only when nothing is filed there; an existing item (even one that
    /// does not decode) is never replaced. It answers whether it stored the value: `true` when
    /// nothing was filed under the key and now the value is, `false` when something already was
    /// and is untouched. Atomic where the backend allows: [`crate::MemorySecrets`] under its lock, the
    /// oo7 file backend under its keyring lock (against every user of the same open keyring in
    /// this process, not against another process or the Secret Service daemon's other clients),
    /// and, at best, read-then-write elsewhere: a second writer can slip in between the read
    /// and the write, and the keyring-core store has a window of one thread job. A caller that
    /// must not lose a value to such a race reads the key again after a `true`.
    ///
    /// The default body is that read-then-write over [`Secrets::get`] and [`Secrets::put`], so a
    /// store written before this method existed keeps compiling; a store with a better primitive
    /// overrides it. Only [`SecretsError::Missing`] means "absent": any other failure to read,
    /// a locked or unavailable store included, is returned and nothing is written.
    fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> impl Future<Output = Result<bool, SecretsError>> + Send {
        async move {
            match self.get(key).await {
                Ok(_) | Err(SecretsError::Unreadable) => Ok(false),
                Err(SecretsError::Missing) => self.put(key, value).await.map(|()| true),
                Err(other) => Err(other),
            }
        }
    }

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
