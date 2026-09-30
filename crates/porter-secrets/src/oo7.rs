//! The Secret Service store through oo7 (Linux), with oo7's file backend keyed by the Secret
//! portal where no Secret Service runs. Frozen interface; the body is not built yet.

use crate::error::SecretsError;
use crate::secrets::Secrets;
use porter_core::{AccountId, Credential, SecretKey};

/// The user's Secret Service collection.
#[derive(Debug, Default)]
pub struct Oo7Secrets;

impl Secrets for Oo7Secrets {
    async fn put(&self, _key: &SecretKey, _value: &Credential) -> Result<(), SecretsError> {
        todo!("store an item with `attributes(key)` through oo7")
    }

    async fn get(&self, _key: &SecretKey) -> Result<Credential, SecretsError> {
        todo!("search by `attributes(key)` through oo7 and decode the credential")
    }

    async fn delete(&self, _key: &SecretKey) -> Result<(), SecretsError> {
        todo!("delete the item found by `attributes(key)`")
    }

    async fn delete_account(&self, _account: &AccountId) -> Result<(), SecretsError> {
        todo!("delete every item whose `account` attribute is this account")
    }
}
