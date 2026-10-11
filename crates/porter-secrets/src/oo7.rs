//! The Secret Service store through oo7 (Linux). oo7 picks its own backend: its file backend,
//! keyed by the Secret portal, inside a sandbox, and the user's default Secret Service
//! collection everywhere else. Where neither answers the store is [`SecretsError::Unavailable`].

use crate::attributes::attributes_in;
use crate::error::SecretsError;
use crate::namespace::Namespace;
use crate::secrets::{PutOutcome, Secrets};
use porter_core::{AccountId, Credential, SecretKey};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

/// The room an encoded credential is written into, so the buffer never grows (a grown buffer
/// leaves its old bytes behind in freed memory). A credential is a few kilobytes at most (two
/// OAuth tokens); a larger one is still written, and only its earlier copies are left unwiped.
const ENCODE_ROOM: usize = 16 * 1024;

/// The bytes `value` is filed as (its JSON), in a buffer wiped when it is dropped. oo7 copies
/// them into its own `Secret`, which wipes itself too.
fn encoded(value: &Credential) -> Result<Zeroizing<Vec<u8>>, SecretsError> {
    let mut buffer = Zeroizing::new(Vec::with_capacity(ENCODE_ROOM));
    serde_json::to_writer(&mut *buffer, value).map_err(|_| SecretsError::Unreadable)?;
    Ok(buffer)
}

/// The user's Secret Service collection, opened on first use and kept, filing under
/// [`Namespace::PORTER`]. [`Oo7Secrets::in_namespace`] files under another.
#[derive(Debug, Clone, Copy, Default)]
pub struct Oo7Secrets;

impl Oo7Secrets {
    /// The user's Secret Service collection, filing under `namespace` (the same collection
    /// [`Oo7Secrets`] opens; only the `service` attribute differs).
    pub fn in_namespace(namespace: Namespace) -> Oo7NamedSecrets {
        Oo7NamedSecrets::new(namespace)
    }
}

/// The user's Secret Service collection, filing under a namespace the host chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Oo7NamedSecrets {
    namespace: Namespace,
}

impl Oo7NamedSecrets {
    /// The user's Secret Service collection, filing under `namespace`.
    pub fn new(namespace: Namespace) -> Self {
        Self { namespace }
    }

    /// The namespace items are filed under.
    pub fn namespace(&self) -> &Namespace {
        &self.namespace
    }
}

/// Secrets over one given oo7 keyring: what [`Oo7Secrets`] does once it has opened its own, and
/// what the tests do over oo7's file backend in a scratch directory.
#[derive(Debug, Clone)]
pub struct Oo7KeyringSecrets {
    keyring: Arc<oo7::Keyring>,
    namespace: Namespace,
}

impl Oo7KeyringSecrets {
    /// Files secrets in `keyring`, under [`Namespace::PORTER`].
    pub fn new(keyring: oo7::Keyring) -> Self {
        Self {
            keyring: Arc::new(keyring),
            namespace: Namespace::PORTER,
        }
    }

    /// The same keyring, filing under `namespace`.
    #[must_use]
    pub fn in_namespace(mut self, namespace: Namespace) -> Self {
        self.namespace = namespace;
        self
    }

    /// The namespace items are filed under.
    pub fn namespace(&self) -> &Namespace {
        &self.namespace
    }
}

/// The opened keyring. A keyring that could not be opened is tried again on the next call: a
/// Secret Service that was not up at the first call (a session whose keyring daemon starts
/// late) is up for the second. One keyring serves every namespace.
static OPENED: Mutex<Option<Arc<oo7::Keyring>>> = Mutex::new(None);

async fn ambient(namespace: &Namespace) -> Result<Oo7KeyringSecrets, SecretsError> {
    let held = OPENED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let keyring = match held {
        Some(keyring) => keyring,
        None => {
            let keyring = Arc::new(oo7::Keyring::new().await.map_err(refused)?);
            *OPENED
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(keyring.clone());
            keyring
        }
    };
    Ok(Oo7KeyringSecrets {
        keyring,
        namespace: namespace.clone(),
    })
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

fn search(namespace: &Namespace, key: &SecretKey) -> HashMap<&'static str, String> {
    let at = attributes_in(namespace, key);
    HashMap::from([
        ("service", at.service),
        ("account", at.account),
        ("purpose", at.purpose),
    ])
}

impl Secrets for Oo7KeyringSecrets {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        let encoded = encoded(value)?;
        let label = format!("{} {}", self.namespace, key.account);
        self.keyring
            .create_item(
                &label,
                &search(&self.namespace, key),
                oo7::Secret::blob(encoded.as_slice()),
                true,
            )
            .await
            .map_err(refused)
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        let encoded = encoded(value)?;
        let label = format!("{} {}", self.namespace, key.account);
        let at = search(&self.namespace, key);
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
                        .create_item(&label, &at, oo7::Secret::blob(encoded.as_slice()), false)
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
                        .create_item(&label, &at, oo7::Secret::blob(encoded.as_slice()), false)
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
            .search_items(&search(&self.namespace, key))
            .await
            .map_err(refused)?;
        let item = items.first().ok_or(SecretsError::Missing)?;
        let secret = item.secret().await.map_err(refused)?;
        serde_json::from_slice(secret.as_bytes()).map_err(|_| SecretsError::Unreadable)
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        self.keyring
            .delete(&search(&self.namespace, key))
            .await
            .map_err(refused)
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        let account = HashMap::from([
            ("service", self.namespace.as_str().to_owned()),
            ("account", account.to_string()),
        ]);
        self.keyring.delete(&account).await.map_err(refused)
    }
}

impl Secrets for Oo7Secrets {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        ambient(&Namespace::PORTER).await?.put(key, value).await
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        ambient(&Namespace::PORTER).await?.get(key).await
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        ambient(&Namespace::PORTER)
            .await?
            .put_if_absent(key, value)
            .await
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        ambient(&Namespace::PORTER).await?.delete(key).await
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        ambient(&Namespace::PORTER)
            .await?
            .delete_account(account)
            .await
    }
}

impl Secrets for Oo7NamedSecrets {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        ambient(&self.namespace).await?.put(key, value).await
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        ambient(&self.namespace).await?.get(key).await
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        ambient(&self.namespace)
            .await?
            .put_if_absent(key, value)
            .await
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        ambient(&self.namespace).await?.delete(key).await
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        ambient(&self.namespace)
            .await?
            .delete_account(account)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{SecretText, UnixSeconds};

    #[test]
    fn a_credential_is_encoded_into_a_wiped_buffer_that_never_grew() {
        // Two Microsoft-sized tokens: well over a kilobyte each.
        let credential = Credential::OAuth {
            access: SecretText::new("a".repeat(3000)),
            refresh: SecretText::new("r".repeat(3000)),
            expires_at: UnixSeconds(1),
        };
        let buffer: Zeroizing<Vec<u8>> = encoded(&credential).expect("encodes");
        assert_eq!(buffer.capacity(), ENCODE_ROOM, "the buffer was not regrown");
        let back: Credential = serde_json::from_slice(&buffer).expect("reads back");
        assert_eq!(back, credential);
    }
}
