use super::*;
use porter_core::AccountState;
use porter_fake::{MemoryStore, llm_account};

fn with(accounts: Vec<porter_core::Account>) -> Registry {
    Registry {
        accounts,
        grants: vec![],
        toggles: vec![],
    }
}

#[test]
fn an_account_coming_changing_and_going_is_said_on_its_state_row() {
    let account = llm_account();
    let id = account.id.clone();
    let mut worse = account.clone();
    worse.state = AccountState::NeedsReauth;
    let row = KeyPath(format!("accounts.{id}.state"));
    let word = |text: &str| toml::Value::String(text.to_owned());
    let cases = [
        ("came", with(vec![]), with(vec![account.clone()]), "ok"),
        (
            "changed",
            with(vec![account.clone()]),
            with(vec![worse]),
            "needs_reauth",
        ),
        ("went", with(vec![account]), with(vec![]), "removed"),
    ];
    for (name, before, after, said) in cases {
        assert_eq!(
            row_news(&before, &after),
            vec![(row.clone(), word(said))],
            "{name}"
        );
    }
}

#[tokio::test]
async fn a_save_that_worked_wakes_the_watcher_and_one_that_failed_does_not() {
    let saved = Arc::new(Notify::new());
    let memory = MemoryStore::default();
    let store = NewsStore::new(memory.clone(), Arc::clone(&saved));
    memory.refusing(true);
    assert!(store.save(&Persisted::empty()).await.is_err());
    memory.refusing(false);
    store.save(&Persisted::empty()).await.expect("saved");
    // The one permit left by the good save is taken at once; a failed save left none behind it.
    tokio::time::timeout(test_kit::GENEROUS, saved.notified())
        .await
        .expect("woken by the save that worked");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), saved.notified())
            .await
            .is_err(),
        "the failed save woke nobody"
    );
}
