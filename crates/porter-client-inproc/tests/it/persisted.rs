//! An app that hosts the service over a store gets back what the store holds: a registry saved by
//! one host is the registry of the next, not an empty one that the next save would overwrite.

use porter_capability::{Access, Delta, StorageScope};
use porter_client::Found;
use porter_client_inproc::{HostConfig, hosted_with_store};
use porter_core::consent::{GrantScope, Usage};
use porter_core::store::Persisted;
use porter_core::{AppId, AppName, DataClass, Isolation};
use porter_fake::{MemoryStore, Scripted, ScriptedSheets, cloud_provider, storage_account};
use porter_match::{Need, StorageNeed};
use porter_secrets::MemorySecrets;
use porter_wire::ParentWindow;

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Photos").expect("app name"),
        isolation: Isolation::Flatpak,
    }
}

fn config(
    store: MemoryStore,
    sheets: ScriptedSheets,
) -> HostConfig<
    porter_fake::FakeProvider,
    MemorySecrets,
    ScriptedSheets,
    porter_ids::clock::SystemClock,
    MemoryStore,
> {
    HostConfig::new(
        app(),
        vec![cloud_provider()],
        MemorySecrets::default(),
        sheets,
    )
    .with_store(store)
}

#[tokio::test]
async fn a_rebuilt_host_loads_the_account_and_the_grant_the_last_one_saved() {
    let store = MemoryStore::holding(Persisted {
        accounts: vec![storage_account()],
        ..Persisted::empty()
    });

    let first = hosted_with_store(config(
        store.clone(),
        ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always)]),
    ))
    .await
    .expect("loads");
    let need = Need::Storage(StorageNeed::new(
        Access::ReadWrite,
        Delta::Poll,
        StorageScope::AppFolder,
        false,
    ));
    let Found::NeedsConsent(offer) = first
        .grants()
        .find(&need, DataClass::Photos, Usage::Interactive)
        .await
        .expect("find")
    else {
        panic!("the account is there to be asked about");
    };
    first
        .grants()
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");
    assert_eq!(store.saves(), 1, "the grant was saved");
    drop(first);

    let second = hosted_with_store(config(store, ScriptedSheets::answering([])))
        .await
        .expect("loads");
    let accounts = second.grants().accounts().await.expect("accounts");
    assert_eq!(
        accounts
            .iter()
            .map(|a| a.account.id.clone())
            .collect::<Vec<_>>(),
        vec![storage_account().id],
        "the account is still there"
    );
    assert_eq!(
        second.grants().grants().await.expect("grants").len(),
        1,
        "and so is the grant"
    );
}
