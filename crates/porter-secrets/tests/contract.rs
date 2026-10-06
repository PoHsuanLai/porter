//! One `Secrets` contract, run over the in-memory store, oo7's file backend in a scratch
//! directory, and the keyring store over keyring-core's mock. Nothing here touches the user's
//! keyring: the Secret Service is never opened.

use porter_core::{AccountId, CapabilityKind, Credential, SecretKey, SecretPurpose, SecretText};
use porter_secrets::{MemorySecrets, PutOutcome, Secrets, SecretsError, StoreSecrets};
use std::sync::Arc;

fn key(account: &str, purpose: SecretPurpose) -> SecretKey {
    SecretKey {
        account: AccountId::parse(account).expect("id"),
        purpose,
    }
}

fn password(text: &str) -> Credential {
    Credential::Password(SecretText::new(text))
}

/// A store under test, and a look at how it filed an item without going through the trait.
trait Fixture {
    type Store: Secrets;
    fn store(&self) -> &Self::Store;
    /// Whether an item with exactly these attributes is filed, as the backend sees it.
    async fn filed(&self, account: &str, purpose: &str) -> Option<bool>;
}

async fn contract<F: Fixture>(fixture: F) {
    let store = fixture.store();

    // A missing item.
    let cloud = key("cloud", SecretPurpose::Password);
    assert_eq!(store.get(&cloud).await, Err(SecretsError::Missing));
    store
        .delete(&cloud)
        .await
        .expect("deleting a missing item succeeds");

    // put, get, replace, delete.
    store.put(&cloud, &password("hunter2")).await.expect("put");
    assert_eq!(store.get(&cloud).await, Ok(password("hunter2")));
    store
        .put(&cloud, &password("hunter3"))
        .await
        .expect("replace");
    assert_eq!(store.get(&cloud).await, Ok(password("hunter3")));

    // The attributes {service: "porter", account, purpose}.
    if let Some(filed) = fixture.filed("cloud", r#"{"kind":"password"}"#).await {
        assert!(filed, "filed under porter/cloud/password");
    }

    // put_if_absent files a missing key once and never replaces what is there.
    let fresh = key("fresh", SecretPurpose::Password);
    assert_eq!(
        store.put_if_absent(&fresh, &password("first")).await,
        Ok(PutOutcome::Stored)
    );
    assert_eq!(
        store.put_if_absent(&fresh, &password("second")).await,
        Ok(PutOutcome::AlreadyThere)
    );
    assert_eq!(store.get(&fresh).await, Ok(password("first")));
    // Present under another purpose or account is not present under this one.
    let sibling = key("fresh", SecretPurpose::ApiKey);
    assert_eq!(
        store
            .put_if_absent(&sibling, &Credential::ApiKey(SecretText::new("k2")))
            .await,
        Ok(PutOutcome::Stored)
    );
    // After a delete it is absent again.
    store.delete(&fresh).await.expect("delete");
    assert_eq!(
        store.put_if_absent(&fresh, &password("third")).await,
        Ok(PutOutcome::Stored)
    );
    assert_eq!(store.get(&fresh).await, Ok(password("third")));
    // Two racing callers: exactly one stores.
    let raced = key("raced", SecretPurpose::Password);
    let (one, two) = (password("a"), password("b"));
    let (a, b) = tokio::join!(
        store.put_if_absent(&raced, &one),
        store.put_if_absent(&raced, &two)
    );
    let outcomes = [a.expect("a"), b.expect("b")];
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == PutOutcome::Stored)
            .count(),
        1,
        "{outcomes:?}"
    );
    store.delete_account(&fresh.account).await.expect("wipe");
    store.delete_account(&raced.account).await.expect("wipe");

    // Every credential shape survives.
    let shapes = [
        (
            SecretPurpose::OAuthRefresh,
            Credential::OAuth {
                access: SecretText::new("a"),
                refresh: SecretText::new("r"),
                expires_at: porter_core::UnixSeconds(9),
            },
        ),
        (
            SecretPurpose::ApiKey,
            Credential::ApiKey(SecretText::new("k")),
        ),
        (
            SecretPurpose::KeyPair,
            Credential::KeyPair {
                access_key: "id".to_owned(),
                secret: SecretText::new("s"),
            },
        ),
        (
            SecretPurpose::ServicePassword(CapabilityKind::Contacts),
            password("dav"),
        ),
    ];
    for (purpose, value) in &shapes {
        let filed = key("cloud", *purpose);
        store.put(&filed, value).await.expect("put");
        assert_eq!(store.get(&filed).await.as_ref(), Ok(value));
    }

    // A large secret, which the keyring store splits over several entries.
    let big = key("big", SecretPurpose::ApiKey);
    let large = Credential::ApiKey(SecretText::new("tok\u{1F600}n".repeat(900)));
    store.put(&big, &large).await.expect("put large");
    assert_eq!(store.get(&big).await, Ok(large));
    store.put(&big, &password("short")).await.expect("shrink");
    assert_eq!(store.get(&big).await, Ok(password("short")));

    // Debug never shows a secret, from the value or from the store.
    let shown = format!("{:?} {:?}", password("hunter3"), store.get(&cloud).await);
    assert!(!shown.contains("hunter3"), "{shown}");

    // delete removes one item and no other.
    store.delete(&cloud).await.expect("delete");
    assert_eq!(store.get(&cloud).await, Err(SecretsError::Missing));
    let kept = key("cloud", SecretPurpose::ApiKey);
    assert_eq!(
        store.get(&kept).await,
        Ok(Credential::ApiKey(SecretText::new("k")))
    );

    // delete_account removes every item of one account and only that account.
    let other = key("other", SecretPurpose::Password);
    store.put(&other, &password("o")).await.expect("put");
    store
        .delete_account(&AccountId::parse("cloud").expect("id"))
        .await
        .expect("wipe");
    for (purpose, _) in &shapes {
        assert_eq!(
            store.get(&key("cloud", *purpose)).await,
            Err(SecretsError::Missing)
        );
    }
    assert_eq!(store.get(&other).await, Ok(password("o")));
    assert_eq!(store.get(&big).await, Ok(password("short")));
    store
        .delete_account(&AccountId::parse("cloud").expect("id"))
        .await
        .expect("wiping an empty account succeeds");
}

struct Memory(MemorySecrets);

impl Fixture for Memory {
    type Store = MemorySecrets;
    fn store(&self) -> &MemorySecrets {
        &self.0
    }
    async fn filed(&self, _: &str, _: &str) -> Option<bool> {
        None
    }
}

#[tokio::test]
async fn memory_keeps_the_contract() {
    contract(Memory(MemorySecrets::default())).await;
}

/// The keyring store over a mock, cutting values at 256 UTF-16 units as Windows would.
struct Mock {
    store: StoreSecrets,
    mock: Arc<keyring_core::CredentialStore>,
}

impl Fixture for Mock {
    type Store = StoreSecrets;
    fn store(&self) -> &StoreSecrets {
        &self.store
    }
    async fn filed(&self, account: &str, purpose: &str) -> Option<bool> {
        let entry = self
            .mock
            .build("porter", &format!("{account}:{purpose}"), None)
            .expect("entry");
        Some(entry.get_password().is_ok())
    }
}

#[tokio::test]
async fn keyring_keeps_the_contract_over_the_mock_store() {
    let mock = keyring_core::mock::Store::new().expect("mock");
    contract(Mock {
        store: StoreSecrets::chunked(mock.clone(), 256),
        mock,
    })
    .await;
}

#[tokio::test]
async fn keyring_without_chunking_keeps_the_contract_too() {
    let mock = keyring_core::mock::Store::new().expect("mock");
    contract(Mock {
        store: StoreSecrets::new(mock.clone()),
        mock,
    })
    .await;
}

#[tokio::test]
async fn the_platform_store_is_the_default_store_once_one_is_set() {
    use porter_secrets::KeyringSecrets;
    keyring_core::set_default_store(keyring_core::mock::Store::new().expect("mock"));
    let filed = key("cloud", SecretPurpose::Password);
    KeyringSecrets
        .put(&filed, &password("p"))
        .await
        .expect("put");
    assert_eq!(KeyringSecrets.get(&filed).await, Ok(password("p")));
    // put_if_absent is forwarded to the store's own, not left to the default body.
    assert_eq!(
        KeyringSecrets.put_if_absent(&filed, &password("q")).await,
        Ok(PutOutcome::AlreadyThere)
    );
    let fresh = key("forwarded", SecretPurpose::Password);
    assert_eq!(
        KeyringSecrets.put_if_absent(&fresh, &password("a")).await,
        Ok(PutOutcome::Stored)
    );
    KeyringSecrets
        .delete_account(&fresh.account)
        .await
        .expect("wipe");
    assert_eq!(KeyringSecrets.get(&filed).await, Ok(password("p")));
    KeyringSecrets
        .delete_account(&filed.account)
        .await
        .expect("wipe");
    assert_eq!(KeyringSecrets.get(&filed).await, Err(SecretsError::Missing));
}

#[cfg(target_os = "linux")]
mod oo7_file {
    use super::*;
    use porter_secrets::Oo7KeyringSecrets;

    type Shared = Arc<tokio::sync::RwLock<Option<oo7::file::Keyring>>>;

    struct File {
        store: Oo7KeyringSecrets,
        shared: Shared,
    }

    impl Fixture for File {
        type Store = Oo7KeyringSecrets;
        fn store(&self) -> &Oo7KeyringSecrets {
            &self.store
        }
        async fn filed(&self, account: &str, purpose: &str) -> Option<bool> {
            let look = oo7::Keyring::File(self.shared.clone());
            let wanted = [
                ("service", "porter"),
                ("account", account),
                ("purpose", purpose),
            ];
            Some(!look.search_items(&wanted).await.expect("search").is_empty())
        }
    }

    /// oo7's file backend at a path in cargo's scratch directory, opened with a test secret.
    async fn scratch(name: &str) -> File {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("oo7-file");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join(format!("{name}-{}.keyring", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let unlocked = oo7::file::UnlockedKeyring::load(&path, oo7::Secret::text("test-secret"))
            .await
            .expect("file keyring");
        let shared: Shared = Arc::new(tokio::sync::RwLock::new(Some(
            oo7::file::Keyring::Unlocked(unlocked),
        )));
        File {
            store: Oo7KeyringSecrets::new(oo7::Keyring::File(shared.clone())),
            shared,
        }
    }

    #[tokio::test]
    async fn oo7_file_backend_keeps_the_contract() {
        contract(scratch("contract").await).await;
    }
}

#[tokio::test]
async fn a_large_secret_is_filed_in_numbered_parts_and_leaves_none_behind() {
    let mock: Arc<keyring_core::CredentialStore> = keyring_core::mock::Store::new().expect("mock");
    let store = StoreSecrets::chunked(mock.clone(), 256);
    let big = key("big", SecretPurpose::ApiKey);
    let large = Credential::ApiKey(SecretText::new("x".repeat(1000)));
    store.put(&big, &large).await.expect("put");
    let part = |n: u8| {
        let name = format!("big:{{\"kind\":\"api_key\"}}#{n}");
        mock.build("porter", &name, None)
            .expect("entry")
            .get_password()
    };
    assert!(part(1).is_ok() && part(4).is_ok());
    store.put(&big, &password("s")).await.expect("shrink");
    assert!(part(1).is_err_and(|e| matches!(e, keyring_core::Error::NoEntry)));
    assert_eq!(store.get(&big).await, Ok(password("s")));
}

/// A store that knows only `get` and `put`, as one written before `put_if_absent` existed.
struct Plain(MemorySecrets, std::sync::atomic::AtomicBool);

impl Secrets for Plain {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        self.0.put(key, value).await
    }
    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        match self.1.load(std::sync::atomic::Ordering::SeqCst) {
            true => Err(SecretsError::Locked),
            false => self.0.get(key).await,
        }
    }
    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        self.0.delete(key).await
    }
    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        self.0.delete_account(account).await
    }
}

#[tokio::test]
async fn the_default_put_if_absent_reads_then_writes_and_a_locked_read_writes_nothing() {
    let store = Plain(MemorySecrets::default(), false.into());
    let k = key("plain", SecretPurpose::Password);
    assert_eq!(
        store.put_if_absent(&k, &password("one")).await,
        Ok(PutOutcome::Stored)
    );
    assert_eq!(
        store.put_if_absent(&k, &password("two")).await,
        Ok(PutOutcome::AlreadyThere)
    );
    assert_eq!(store.get(&k).await, Ok(password("one")));
    let other = key("plain2", SecretPurpose::Password);
    store.1.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        store.put_if_absent(&other, &password("x")).await,
        Err(SecretsError::Locked)
    );
    store.1.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(store.get(&other).await, Err(SecretsError::Missing));
}
