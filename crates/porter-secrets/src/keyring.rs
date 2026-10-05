//! The platform keyring store for an app that hosts porter's core itself on macOS or Windows
//! (the Apple keychain, the Windows credential manager). Blocking keyring calls run on a
//! dedicated thread and resolve a oneshot, because this crate may not reach a runtime and a
//! blocking call on an async executor once panicked mailo. Windows limits an item's size, so a
//! credential is stored in chunks under numbered attributes. Frozen interface; the body is not
//! built yet.

use crate::error::SecretsError;
use crate::secrets::Secrets;
use porter_core::{AccountId, Credential, SecretKey};

/// The user's platform keyring.
#[derive(Debug, Default)]
pub struct KeyringSecrets;

impl Secrets for KeyringSecrets {
    async fn put(&self, _key: &SecretKey, _value: &Credential) -> Result<(), SecretsError> {
        todo!(
            "encode the credential, chunk it, store each chunk under `attributes(key)` and its index"
        )
    }

    async fn get(&self, _key: &SecretKey) -> Result<Credential, SecretsError> {
        todo!("read the chunks under `attributes(key)`, join them and decode the credential")
    }

    async fn delete(&self, _key: &SecretKey) -> Result<(), SecretsError> {
        todo!("delete every chunk under `attributes(key)`; a missing item succeeds")
    }

    async fn delete_account(&self, _account: &AccountId) -> Result<(), SecretsError> {
        todo!("delete every item whose `account` attribute is this account")
    }
}
