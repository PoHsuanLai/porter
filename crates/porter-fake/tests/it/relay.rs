//! Opening a relay: the grant is spent on the registry as it is when the plan is ready, so two
//! openers of one `Once` grant make one relay, and a plan that could not be made spends nothing.

use porter_capability::{Access, Delta, StorageScope};
use porter_core::consent::{GrantScope, Usage};
use porter_core::{AccountId, AppId, AppName, Credential, DataClass, Isolation, SecretKey};
use porter_core::{EndpointUrl, GrantId};
use porter_fake::{
    FakeProvider, Scripted, ScriptedSheets, cloud_provider, fake_service, llm_provider,
    mail_provider,
};
use porter_ids::clock::FixedClock;
use porter_match::{Need, StorageNeed};
use porter_secrets::{MemorySecrets, Secrets, SecretsError};
use porter_service::AccountService;
use porter_wire::{AccountsReply, ParentWindow, Refusal};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Barrier;

/// Secrets whose reads wait for each other (so two openers are both past their first check
/// before either goes on), or fail.
struct Meeting {
    inner: MemorySecrets,
    both_read: Barrier,
    failing: Arc<AtomicBool>,
}

impl Secrets for Meeting {
    async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
        self.inner.put(key, value).await
    }

    async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
        if self.failing.load(Ordering::SeqCst) {
            return Err(SecretsError::Unavailable);
        }
        let credential = self.inner.get(key).await;
        self.both_read.wait().await;
        credential
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        self.inner.delete(key).await
    }

    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        self.inner.delete_account(account).await
    }
}

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Photos").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

type Svc = AccountService<FakeProvider, Meeting, ScriptedSheets, FixedClock>;

/// A service holding one `Once` grant to the storage account, whose secrets are `Meeting`.
async fn holding_a_once_grant() -> (Svc, GrantId, EndpointUrl, Arc<AtomicBool>) {
    let failing = Arc::new(AtomicBool::new(false));
    let first = fake_service(ScriptedSheets::answering([Scripted::AllowFirst(
        GrantScope::Once,
    )]))
    .await;
    let need = Need::Storage(StorageNeed::new(
        Access::ReadWrite,
        Delta::Poll,
        StorageScope::AppFolder,
        false,
    ));
    let asked = first
        .choose(
            &app(),
            need,
            (DataClass::Files, Usage::Interactive),
            None,
            &ParentWindow::Unparented,
            None,
        )
        .await;
    let AccountsReply::Chosen(chosen) = asked else {
        panic!("not chosen: {asked:?}");
    };
    let secrets = Meeting {
        inner: MemorySecrets::default(),
        both_read: Barrier::new(2),
        failing: Arc::clone(&failing),
    };
    let key = SecretKey {
        account: chosen.account.clone(),
        purpose: porter_core::SecretPurpose::Password,
    };
    secrets
        .put(
            &key,
            &Credential::Password(porter_core::SecretText::new("pw")),
        )
        .await
        .expect("filed");
    let service = AccountService::new(
        vec![cloud_provider(), mail_provider(), llm_provider()],
        first.registry(),
        secrets,
        ScriptedSheets::default(),
        FixedClock(porter_fake::NOW),
    );
    (
        service,
        chosen.grant,
        chosen.endpoints[0].url.clone(),
        failing,
    )
}

#[tokio::test]
async fn two_openers_of_a_once_grant_make_one_relay() {
    let (service, grant, endpoint, _) = holding_a_once_grant().await;
    let me = app();
    let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(
            service.open_authenticated(&me, &grant, &endpoint),
            service.open_authenticated(&me, &grant, &endpoint),
        )
    })
    .await
    .expect("both finish");
    let (opened, refused): (Vec<_>, Vec<_>) = [a, b].into_iter().partition(Result::is_ok);
    assert_eq!(opened.len(), 1, "one relay");
    assert_eq!(refused[0].as_ref().err(), Some(&Refusal::UnknownGrant));
    assert!(service.registry().grants.is_empty(), "the grant is spent");
}

#[tokio::test]
async fn a_plan_that_could_not_be_made_spends_nothing() {
    let (service, grant, endpoint, failing) = holding_a_once_grant().await;
    failing.store(true, Ordering::SeqCst);
    let refused = service.open_authenticated(&app(), &grant, &endpoint).await;
    assert_eq!(refused.err(), Some(Refusal::Unavailable));
    assert_eq!(service.registry().grants.len(), 1, "still held");
}
