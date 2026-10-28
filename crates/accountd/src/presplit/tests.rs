use super::*;
use porter_capability::StorageScope;
use porter_capability::{CapabilityKind, Toggle};
use porter_core::GrantId;
use porter_core::store::{AccountToggle, Persisted};
use porter_core::{AppId, AppName, AuthKind, DataClass, Isolation, SpaceScope};
use porter_core::{
    GrantKey,
    consent::{Decision, Grant, GrantScope, Usage},
};
use porter_fake::{llm_account, mail_account, storage_account};
use porter_ids::UnixSeconds;
use test_kit::Scratch;

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
            reach: StorageScope::AppFolder,
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
    let scratch = Scratch::new("presplit");
    let store = FileStore::new(scratch.path().to_path_buf());
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
    let scratch = Scratch::new("presplit");
    let store = FileStore::new(scratch.path().to_path_buf());
    let copy = pre_split_path(&store);
    std::fs::write(&copy, "the first copy").expect("copy");
    store.save(&old_registry()).await.expect("saved");
    load_identities(&store).await.expect("loaded");
    assert_eq!(
        std::fs::read_to_string(&copy).expect("the copy"),
        "the first copy"
    );

    let clean = Scratch::new("presplit");
    let store = FileStore::new(clean.path().to_path_buf());
    let mut only = Persisted::empty();
    only.accounts = vec![storage_account()];
    only.accounts[0].auth = AuthKind::Password;
    store.save(&only).await.expect("saved");
    load_identities(&store).await.expect("loaded");
    assert!(!pre_split_path(&store).exists());
}
