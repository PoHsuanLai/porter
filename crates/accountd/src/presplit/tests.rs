use super::*;
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::store::{AccountToggle, Persisted};
use porter_core::{
    AppId, AppName, AuthKind, CapabilityKind, DataClass, GrantId, Isolation, SpaceScope, Toggle,
    UnixSeconds,
};
use porter_fake::{llm_account, mail_account, storage_account};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory under the system temp directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "accountd-presplit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What a registry held before the split: an identity, an AI key, a grant and a switch.
fn old_registry() -> Persisted {
    let mut identity = mail_account();
    identity.auth = AuthKind::OAuthPkce;
    let mut key = llm_account();
    key.auth = AuthKind::ApiKey;
    let mut stored = Persisted::empty();
    stored.grants.push(Grant {
        id: GrantId::parse("g1").expect("id"),
        key: GrantKey {
            app: AppId {
                name: AppName::parse("org.quire.Photos").expect("name"),
                isolation: Isolation::Flatpak,
            },
            account: key.id.clone(),
            kind: CapabilityKind::Llm,
            class: DataClass::Photos,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    });
    stored.toggles.push(AccountToggle {
        account: identity.id.clone(),
        kind: CapabilityKind::Mail,
        toggle: Toggle::Off,
    });
    stored.accounts = vec![identity, key];
    stored
}

#[tokio::test]
async fn an_old_file_loads_as_its_identities_and_is_copied_once_unchanged() {
    let scratch = Scratch::new();
    let store = FileStore::new(scratch.0.clone());
    let old = old_registry();
    store.save(&old).await.expect("saved");
    let before = std::fs::read(store.path()).expect("old file");

    let loaded = load_identities(&store).await.expect("loaded");
    assert_eq!(
        loaded
            .accounts
            .iter()
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>(),
        ["fake-mail"]
    );
    assert!(loaded.grants.is_empty() && loaded.toggles.is_empty());
    let copy = pre_split_path(&store);
    assert_eq!(std::fs::read(&copy).expect("the copy"), before);

    // A later load, after the pruned registry was saved, leaves the copy as it was.
    store.save(&loaded).await.expect("saved");
    let again = load_identities(&store).await.expect("loaded");
    assert_eq!(again, loaded);
    assert_eq!(std::fs::read(&copy).expect("the copy"), before);
}

#[tokio::test]
async fn a_copy_that_is_there_is_never_overwritten_and_a_clean_file_makes_none() {
    let scratch = Scratch::new();
    let store = FileStore::new(scratch.0.clone());
    let copy = pre_split_path(&store);
    std::fs::write(&copy, "the first copy").expect("copy");
    store.save(&old_registry()).await.expect("saved");
    load_identities(&store).await.expect("loaded");
    assert_eq!(
        std::fs::read_to_string(&copy).expect("the copy"),
        "the first copy"
    );

    let clean = Scratch::new();
    let store = FileStore::new(clean.0.clone());
    let mut only = Persisted::empty();
    only.accounts = vec![storage_account()];
    only.accounts[0].auth = AuthKind::Password;
    store.save(&only).await.expect("saved");
    load_identities(&store).await.expect("loaded");
    assert!(!pre_split_path(&store).exists());
}
