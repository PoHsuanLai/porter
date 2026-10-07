//! Where a key comes from: the app's, never ours. porter-client holds none of them: a key is
//! asked for when a session opens (to know whether the engine may be used at all, and dropped at
//! once) and again for each turn (copied once into that turn's request header, and dropped with
//! it). It is in no field of the host, no event, no log line.

use porter_core::{AccountId, SecretText};
use std::future::Future;
use std::sync::Arc;

/// The app's access to the API keys of its accounts (its keychain, its settings, a vault).
pub trait KeySource: Send + Sync {
    /// The key for `account`, or `None` when the person has given none (or it cannot be read
    /// now: a locked keychain is a key that is not there, and the session says `NeedsGrant`).
    fn key(&self, account: &AccountId) -> impl Future<Output = Option<SecretText>> + Send;
}

/// No keys: a host whose engines all run on this computer without one.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoKeys;

impl KeySource for NoKeys {
    async fn key(&self, _account: &AccountId) -> Option<SecretText> {
        None
    }
}

impl<T: KeySource> KeySource for Arc<T> {
    fn key(&self, account: &AccountId) -> impl Future<Output = Option<SecretText>> + Send {
        (**self).key(account)
    }
}
