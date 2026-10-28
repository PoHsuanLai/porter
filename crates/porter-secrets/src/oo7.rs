//! The Secret Service store through oo7 (Linux). oo7 picks its own backend: its file backend,
//! keyed by the Secret portal, inside a sandbox, and the user's default Secret Service
//! collection everywhere else. Where neither answers the store is [`SecretsError::Unavailable`].

use crate::attributes::attributes_in;
use crate::error::SecretsError;
use crate::namespace::Namespace;
use crate::secrets::{PutOutcome, Secrets};
use porter_core::AccountId;
use porter_core::{Credential, SecretKey};
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
static OPENED: Held<oo7::Keyring> = Held::new();

/// A store opened on first use and kept until it stops answering.
///
/// A kept Secret Service keyring holds a session with the daemon that opened it. When that daemon
/// goes (the desktop session ends, and the next one starts a daemon of its own), every call on
/// the kept keyring is refused, and a process that outlives the session, a sync service, would
/// never reach the new daemon. So a call refused as [`SecretsError::Unavailable`] forgets the
/// keyring, and a call that used a kept one is tried once more on a freshly opened one.
struct Held<K> {
    slot: Mutex<Option<Arc<K>>>,
}

impl<K> Held<K> {
    const fn new() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<Arc<K>>> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn open<O, Opening>(&self, open: &O) -> Result<Arc<K>, SecretsError>
    where
        O: Fn() -> Opening,
        Opening: Future<Output = Result<K, SecretsError>>,
    {
        let keyring = Arc::new(open().await?);
        *self.slot() = Some(keyring.clone());
        Ok(keyring)
    }

    /// Forgets `keyring`, unless another call has already put a newer one in its place.
    fn forget(&self, keyring: &Arc<K>) {
        let mut slot = self.slot();
        if slot.as_ref().is_some_and(|held| Arc::ptr_eq(held, keyring)) {
            *slot = None;
        }
    }

    /// `call` on the kept keyring, or on one opened with `open` when none is kept.
    async fn run<T, O, Opening, C, Calling>(&self, open: O, call: C) -> Result<T, SecretsError>
    where
        O: Fn() -> Opening,
        Opening: Future<Output = Result<K, SecretsError>>,
        C: Fn(Arc<K>) -> Calling,
        Calling: Future<Output = Result<T, SecretsError>>,
    {
        let held = self.slot().clone();
        let kept = held.is_some();
        let keyring = match held {
            Some(keyring) => keyring,
            None => self.open(&open).await?,
        };
        let answer = call(keyring.clone()).await;
        if !matches!(answer, Err(SecretsError::Unavailable)) {
            return answer;
        }
        self.forget(&keyring);
        if !kept {
            return answer;
        }
        let fresh = self.open(&open).await?;
        let answer = call(fresh.clone()).await;
        if matches!(answer, Err(SecretsError::Unavailable)) {
            self.forget(&fresh);
        }
        answer
    }
}

/// `call` on the user's keyring, filing under `namespace`.
async fn ambient<T, C, Calling>(namespace: &Namespace, call: C) -> Result<T, SecretsError>
where
    C: Fn(Oo7KeyringSecrets) -> Calling,
    Calling: Future<Output = Result<T, SecretsError>>,
{
    OPENED
        .run(
            || async { oo7::Keyring::new().await.map_err(refused) },
            |keyring| {
                call(Oo7KeyringSecrets {
                    keyring,
                    namespace: namespace.clone(),
                })
            },
        )
        .await
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
        ambient(
            &Namespace::PORTER,
            |s| async move { s.put(key, value).await },
        )
        .await
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        ambient(&Namespace::PORTER, |s| async move { s.get(key).await }).await
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        ambient(&Namespace::PORTER, |s| async move {
            s.put_if_absent(key, value).await
        })
        .await
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        ambient(&Namespace::PORTER, |s| async move { s.delete(key).await }).await
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        ambient(&Namespace::PORTER, |s| async move {
            s.delete_account(account).await
        })
        .await
    }
}

impl Secrets for Oo7NamedSecrets {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        ambient(&self.namespace, |s| async move { s.put(key, value).await }).await
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        ambient(&self.namespace, |s| async move { s.get(key).await }).await
    }

    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<PutOutcome, SecretsError> {
        ambient(&self.namespace, |s| async move {
            s.put_if_absent(key, value).await
        })
        .await
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        ambient(&self.namespace, |s| async move { s.delete(key).await }).await
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        ambient(&self.namespace, |s| async move {
            s.delete_account(account).await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::SecretText;
    use porter_ids::UnixSeconds;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A daemon that answers only the keyrings opened since it started: the `n`th open is
    /// keyring `n`, and a keyring older than `started` is refused as unavailable.
    struct Daemon {
        opens: AtomicU32,
        started: AtomicU32,
    }

    impl Daemon {
        fn new() -> Self {
            Self {
                opens: AtomicU32::new(0),
                started: AtomicU32::new(1),
            }
        }

        async fn open(&self) -> Result<u32, SecretsError> {
            Ok(self.opens.fetch_add(1, Ordering::SeqCst) + 1)
        }

        /// The daemon goes and a new one starts: every keyring opened so far is refused.
        fn restart(&self) {
            self.started
                .store(self.opens.load(Ordering::SeqCst) + 1, Ordering::SeqCst);
        }

        async fn read(&self, keyring: Arc<u32>) -> Result<u32, SecretsError> {
            match *keyring >= self.started.load(Ordering::SeqCst) {
                true => Ok(*keyring),
                false => Err(SecretsError::Unavailable),
            }
        }
    }

    #[tokio::test]
    async fn a_kept_keyring_the_daemon_no_longer_answers_is_opened_again() {
        let held = Held::new();
        let daemon = Daemon::new();
        let read = || held.run(|| daemon.open(), |k| daemon.read(k));
        assert_eq!(read().await, Ok(1));
        assert_eq!(read().await, Ok(1), "the keyring is kept between calls");
        daemon.restart();
        assert_eq!(read().await, Ok(2), "the same call reaches the new daemon");
        assert_eq!(read().await, Ok(2));
        assert_eq!(daemon.opens.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_keyring_that_answers_missing_or_locked_is_kept() {
        let held = Held::new();
        let opens = AtomicU32::new(0);
        let open = || async {
            opens.fetch_add(1, Ordering::SeqCst);
            Ok::<_, SecretsError>(())
        };
        for refusal in [SecretsError::Missing, SecretsError::Locked] {
            let got = held.run(open, |_| async { Err::<(), _>(refusal) }).await;
            assert_eq!(got, Err(refusal));
        }
        assert_eq!(opens.load(Ordering::SeqCst), 1, "opened once, never again");
    }

    #[tokio::test]
    async fn a_daemon_that_is_down_is_tried_once_per_call_and_again_on_the_next() {
        let held = Held::new();
        let opens = AtomicU32::new(0);
        let open = || async {
            opens.fetch_add(1, Ordering::SeqCst);
            Ok::<_, SecretsError>(())
        };
        let down = |_| async { Err::<(), _>(SecretsError::Unavailable) };
        assert_eq!(held.run(open, down).await, Err(SecretsError::Unavailable));
        assert_eq!(
            opens.load(Ordering::SeqCst),
            1,
            "a fresh keyring is not retried"
        );
        assert_eq!(held.run(open, down).await, Err(SecretsError::Unavailable));
        assert_eq!(opens.load(Ordering::SeqCst), 2, "the next call opens again");
    }

    #[test]
    fn a_credential_is_encoded_into_a_wiped_buffer_that_never_grew() {
        // Two Microsoft-sized tokens: well over a kilobyte each.
        let credential = Credential::OAuth {
            access: SecretText::new("a".repeat(3000)),
            refresh: SecretText::new("r".repeat(3000)),
            expires_at: UnixSeconds(1),
            granted: Default::default(),
        };
        let buffer: Zeroizing<Vec<u8>> = encoded(&credential).expect("encodes");
        assert_eq!(buffer.capacity(), ENCODE_ROOM, "the buffer was not regrown");
        let back: Credential = serde_json::from_slice(&buffer).expect("reads back");
        assert_eq!(back, credential);
    }
}
