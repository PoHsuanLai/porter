//! The import over scratch directories and in-memory secrets with fake keys: nothing real.

use super::*;
use porter_capability::{CapabilityKind, StorageScope};
use porter_core::consent::{Decision, Grant, GrantScope, Usage};
use porter_core::store::AccountToggle;
use porter_core::{
    Account, AppId, AppName, AuthKind, DataClass, GrantId, GrantKey, Isolation, ProviderId,
    SecretPurpose, SecretText, SpaceScope,
};
use porter_secrets::MemorySecrets;
use std::sync::atomic::{AtomicBool, Ordering};
use test_kit::Scratch;

const KEY: &str = "sk-fake-0001-NOT-A-REAL-KEY";
const OTHER: &str = "sk-fake-0002-NOT-A-REAL-KEY";

/// A secret store that can be told to refuse.
#[derive(Debug, Default)]
struct Switch {
    inner: MemorySecrets,
    refusing: AtomicBool,
}

impl Switch {
    fn refuse(&self, on: bool) {
        self.refusing.store(on, Ordering::SeqCst);
    }

    fn check(&self) -> Result<(), SecretsError> {
        match self.refusing.load(Ordering::SeqCst) {
            true => Err(SecretsError::Unavailable),
            false => Ok(()),
        }
    }
}

impl Secrets for Switch {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        self.check()?;
        self.inner.put(key, value).await
    }
    async fn put_if_absent(
        &self,
        key: &SecretKey,
        value: &Credential,
    ) -> Result<bool, SecretsError> {
        self.check()?;
        self.inner.put_if_absent(key, value).await
    }
    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        self.check()?;
        self.inner.get(key).await
    }
    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        self.check()?;
        self.inner.delete(key).await
    }
    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        self.check()?;
        self.inner.delete_account(account).await
    }
}

use porter_core::Credential;

fn key_of(account: &Account) -> SecretKey {
    SecretKey {
        account: account.id.clone(),
        purpose: SecretPurpose::ApiKey,
    }
}

fn credential(text: &str) -> Credential {
    Credential::ApiKey(SecretText::new(text))
}

/// An AI key account of the given id.
fn account(id: &str, provider: &str) -> Account {
    let mut account = porter_fake::llm_account();
    account.id = AccountId::parse(id).expect("id");
    account.provider = ProviderId::parse(provider).expect("provider");
    account.auth = AuthKind::ApiKey;
    account
}

fn permit(id: &str, account: &Account) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("id"),
        key: GrantKey {
            app: AppId {
                name: AppName::parse("org.example.Notes").expect("name"),
                isolation: Isolation::Unsandboxed,
            },
            account: account.id.clone(),
            kind: CapabilityKind::Llm,
            class: DataClass::Prompt,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
            reach: StorageScope::AppFolder,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: porter_ids::UnixSeconds(5),
    }
}

/// Scratch directories and the stores of one import.
struct Rig {
    scratch: Scratch,
    old_secrets: Switch,
    secrets: Switch,
}

impl Rig {
    fn new() -> Self {
        Self {
            scratch: Scratch::new("ai-import"),
            old_secrets: Switch::default(),
            secrets: Switch::default(),
        }
    }

    fn old(&self) -> OldStore {
        OldStore {
            registry_dir: self.scratch.join("inferd/keys"),
            audit: self.scratch.join("inferd/keys-audit.jsonl"),
        }
    }

    fn store(&self) -> FileStore {
        FileStore::new(self.scratch.join("accountd/porter"))
    }

    fn audit(&self) -> PathBuf {
        self.scratch.join("accountd/audit.jsonl")
    }

    fn marker(&self) -> PathBuf {
        self.scratch.join("accountd/ai-import.json")
    }

    /// What accountd's registry holds now.
    async fn mine(&self) -> Persisted {
        self.store().load().await.expect("loaded")
    }

    async fn run(&self) -> Result<Outcome, ImportError> {
        let (old, store, audit, marker) = (self.old(), self.store(), self.audit(), self.marker());
        Import {
            old: &old,
            old_secrets: &self.old_secrets,
            secrets: &self.secrets,
            store: &store,
            audit: &audit,
            marker: &marker,
        }
        .run()
        .await
    }

    /// inferd's old store: the accounts, a permit on the first and a switch, their keys, and two
    /// audit lines.
    async fn seed(&self, accounts: &[Account]) -> Persisted {
        let mut persisted = Persisted::empty();
        persisted.accounts = accounts.to_vec();
        persisted.grants = vec![permit("g.1", &accounts[0])];
        persisted.toggles = vec![AccountToggle {
            account: accounts[0].id.clone(),
            kind: CapabilityKind::Llm,
            toggle: porter_capability::Toggle::Off,
        }];
        FileStore::new(self.old().registry_dir)
            .save(&persisted)
            .await
            .expect("seeded");
        for account in accounts {
            self.old_secrets
                .put(&key_of(account), &credential(KEY))
                .await
                .expect("key");
        }
        std::fs::write(
            self.old().audit,
            "{\"at\":1,\"event\":\"a\"}\n{\"at\":2,\"event\":\"b\"}\n",
        )
        .expect("audit");
        persisted
    }
}

#[tokio::test]
async fn the_store_is_copied_verified_recorded_and_the_old_secrets_removed() {
    let rig = Rig::new();
    let accounts = [
        account("openrouter", "openrouter"),
        account("a.b", "openai"),
    ];
    let old = rig.seed(&accounts).await;
    let Outcome::Imported(report) = rig.run().await.expect("imported") else {
        panic!("imported");
    };
    assert_eq!(
        report,
        Report {
            accounts: 2,
            grants: 1,
            toggles: 1,
            secrets: 2,
            conflicts: 0,
            audit_lines: 2,
        }
    );
    let loaded = rig.store().load().await.expect("loaded");
    assert_eq!(loaded.accounts, old.accounts);
    assert_eq!(loaded.grants, old.grants);
    assert_eq!(loaded.toggles, old.toggles);
    for account in &accounts {
        assert_eq!(
            rig.secrets.get(&key_of(account)).await.expect("copied"),
            credential(KEY)
        );
        assert_eq!(
            rig.old_secrets.get(&key_of(account)).await.map(drop),
            Err(SecretsError::Missing),
            "the old item is gone"
        );
    }
    let audit = std::fs::read_to_string(rig.audit()).expect("audit");
    assert_eq!(audit.lines().count(), 2);
    let marker = ImportMarker::read(&rig.marker())
        .expect("read")
        .expect("there");
    assert!(marker.secrets_removed);
    assert_eq!((marker.accounts, marker.grants, marker.secrets), (2, 1, 2));
    let bytes = std::fs::read(FileStore::new(rig.old().registry_dir).path()).expect("old file");
    assert!(marker.covers(&bytes));
    // No fake key is in anything that was printed, saved to the registry or written to the audit.
    let shown = format!("{report:?}{marker:?}{audit}");
    let registry = std::fs::read_to_string(rig.store().path()).expect("registry");
    for text in [shown, registry] {
        assert!(!text.contains("sk-fake"), "{text}");
    }
}

#[tokio::test]
async fn a_second_run_changes_nothing_and_a_changed_old_registry_adds_only_what_is_new() {
    let rig = Rig::new();
    let first = account("openrouter", "openrouter");
    rig.seed(std::slice::from_ref(&first)).await;
    assert!(matches!(rig.run().await, Ok(Outcome::Imported(_))));
    let after = rig.mine().await;
    assert_eq!(rig.run().await.expect("again"), Outcome::AlreadyImported);
    assert_eq!(rig.mine().await, after);
    assert_eq!(
        std::fs::read_to_string(rig.audit())
            .expect("audit")
            .lines()
            .count(),
        2,
        "the audit lines are not appended twice"
    );
    // inferd ran again and gave one more permit.
    let second = account("anthropic", "anthropic");
    let mut changed = FileStore::new(rig.old().registry_dir)
        .load()
        .await
        .expect("old");
    changed.accounts.push(second.clone());
    changed.grants.push(permit("g.2", &second));
    FileStore::new(rig.old().registry_dir)
        .save(&changed)
        .await
        .expect("changed");
    rig.old_secrets
        .put(&key_of(&second), &credential(OTHER))
        .await
        .expect("key");
    let Outcome::Imported(report) = rig.run().await.expect("imported") else {
        panic!("imported");
    };
    assert_eq!((report.accounts, report.grants, report.secrets), (1, 1, 1));
    assert_eq!(rig.mine().await.grants.len(), 2);
}

#[tokio::test]
async fn an_id_that_means_another_account_refuses_the_import_with_nothing_changed() {
    let rig = Rig::new();
    let theirs = account("shared", "openrouter");
    rig.seed(std::slice::from_ref(&theirs)).await;
    let mut mine = Persisted::empty();
    let mut other = account("shared", "anthropic");
    other.label = porter_core::AccountLabel("Mine".into());
    mine.accounts.push(other);
    rig.store().save(&mine).await.expect("mine");
    assert!(matches!(
        rig.run().await,
        Err(ImportError::Conflict(id)) if id.as_str() == "shared"
    ));
    assert_eq!(rig.mine().await, mine);
    assert_eq!(
        rig.secrets.get(&key_of(&theirs)).await.map(drop),
        Err(SecretsError::Missing),
        "no key was filed"
    );
    assert!(rig.old_secrets.get(&key_of(&theirs)).await.is_ok());
    assert!(ImportMarker::read(&rig.marker()).expect("read").is_none());
}

#[tokio::test]
async fn a_key_accountd_holds_already_is_kept_and_counted() {
    let rig = Rig::new();
    let one = account("openrouter", "openrouter");
    rig.seed(std::slice::from_ref(&one)).await;
    rig.secrets
        .put(&key_of(&one), &credential(OTHER))
        .await
        .expect("held");
    let Outcome::Imported(report) = rig.run().await.expect("imported") else {
        panic!("imported");
    };
    assert_eq!((report.secrets, report.conflicts), (0, 1));
    assert_eq!(
        rig.secrets.get(&key_of(&one)).await.expect("kept"),
        credential(OTHER)
    );
}

#[tokio::test]
async fn a_secret_store_that_refuses_leaves_everything_of_inferds_and_accountds_untouched() {
    let rig = Rig::new();
    let one = account("openrouter", "openrouter");
    rig.seed(std::slice::from_ref(&one)).await;
    rig.secrets.refuse(true);
    assert!(matches!(rig.run().await, Err(ImportError::Secrets(_))));
    rig.secrets.refuse(false);
    assert!(rig.mine().await.accounts.is_empty());
    assert!(rig.old_secrets.get(&key_of(&one)).await.is_ok());
    assert!(ImportMarker::read(&rig.marker()).expect("read").is_none());
    // The next start finishes it.
    assert!(matches!(rig.run().await, Ok(Outcome::Imported(_))));
}

#[tokio::test]
async fn old_items_that_could_not_be_removed_are_removed_at_the_next_start() {
    let rig = Rig::new();
    let one = account("openrouter", "openrouter");
    rig.seed(std::slice::from_ref(&one)).await;
    assert!(matches!(rig.run().await, Ok(Outcome::Imported(_))));
    // As if the removal had failed: the record says so and the item is back.
    let mut marker = ImportMarker::read(&rig.marker())
        .expect("read")
        .expect("there");
    marker.secrets_removed = false;
    marker.write(&rig.marker()).expect("written");
    rig.old_secrets
        .put(&key_of(&one), &credential(KEY))
        .await
        .expect("back");
    assert_eq!(rig.run().await.expect("again"), Outcome::AlreadyImported);
    assert_eq!(
        rig.old_secrets.get(&key_of(&one)).await.map(drop),
        Err(SecretsError::Missing)
    );
    assert!(
        ImportMarker::read(&rig.marker())
            .expect("read")
            .expect("there")
            .secrets_removed
    );
}

#[tokio::test]
async fn an_account_whose_key_is_gone_is_still_copied_and_no_inferd_registry_is_nothing() {
    let rig = Rig::new();
    assert_eq!(rig.run().await.expect("nothing"), Outcome::NothingToImport);
    let one = account("openrouter", "openrouter");
    rig.seed(std::slice::from_ref(&one)).await;
    rig.old_secrets.delete(&key_of(&one)).await.expect("gone");
    let Outcome::Imported(report) = rig.run().await.expect("imported") else {
        panic!("imported");
    };
    assert_eq!((report.accounts, report.secrets), (1, 0));
}

#[tokio::test]
async fn an_existing_audit_file_is_appended_to_not_replaced() {
    let rig = Rig::new();
    rig.seed(&[account("openrouter", "openrouter")]).await;
    std::fs::create_dir_all(rig.audit().parent().expect("dir")).expect("dir");
    std::fs::write(rig.audit(), "{\"at\":0}\n").expect("mine");
    assert!(matches!(rig.run().await, Ok(Outcome::Imported(_))));
    let text = std::fs::read_to_string(rig.audit()).expect("audit");
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        [
            "{\"at\":0}",
            "{\"at\":1,\"event\":\"a\"}",
            "{\"at\":2,\"event\":\"b\"}"
        ]
    );
}
