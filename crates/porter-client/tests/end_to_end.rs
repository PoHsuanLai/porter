//! An app on the in-process transport, over the fake providers: it finds an account by
//! capability, is asked for consent, and gets a token; a refusal sticks; a once-grant is spent.

use porter_client::{Accounts, ClientError, ConsentOffer, Found, InProcess, NoAccount};
use porter_core::capability::{Access, Delta, QuotaReport, StorageScope};
use porter_core::consent::{GrantScope, Usage};
use porter_core::need::StorageNeed;
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{AccountId, AppId, AppName, Audience, DataClass, Isolation, Need};
use porter_fake::{FakeService, Scripted, ScriptedPrompter, fake_service};
use std::sync::Arc;

type App = Accounts<
    InProcess<
        porter_fake::FakeProvider,
        porter_secrets::MemorySecrets,
        ScriptedPrompter,
        porter_fake::FixedClock,
    >,
>;

fn app(service: &Arc<FakeService>, name: &str) -> App {
    let id = AppId {
        name: AppName::parse(name).expect("app name"),
        isolation: Isolation::Flatpak,
    };
    Accounts::over(InProcess::new(Arc::clone(service), id))
}

fn storage(delta: Delta) -> Need {
    Need::Storage(StorageNeed {
        access: Access::ReadWrite,
        delta,
        scope: StorageScope::AppFolder,
        quota: QuotaReport::Unreported,
    })
}

fn needs_consent(found: Found) -> ConsentOffer {
    match found {
        Found::NeedsConsent(offer) => offer,
        other => panic!("expected NeedsConsent, got {other:?}"),
    }
}

#[tokio::test]
async fn a_storage_need_finds_the_fake_cloud_after_consent() {
    let prompter = ScriptedPrompter::answering([Scripted::AllowFirst(GrantScope::Always)]);
    let log = prompter.log();
    let service = Arc::new(fake_service(prompter).await);
    let photos = app(&service, "org.quire.Photos");
    let need = storage(Delta::Poll);

    let offer = needs_consent(
        photos
            .find(&need, DataClass::Photos, Usage::Interactive)
            .await
            .expect("find"),
    );
    let chosen = photos
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");

    let storage_id = AccountId::parse("fake-storage").expect("id");
    assert_eq!(chosen.account, storage_id);
    let offered: Vec<Vec<AccountId>> = log
        .asked()
        .iter()
        .map(|ask| ask.accounts.iter().map(|c| c.account.clone()).collect())
        .collect();
    assert_eq!(
        offered,
        vec![vec![storage_id.clone()]],
        "only the storage account is offered"
    );

    match photos
        .find(&need, DataClass::Photos, Usage::Interactive)
        .await
        .expect("find")
    {
        Found::One(candidate) => assert_eq!(candidate, chosen),
        other => panic!("expected One, got {other:?}"),
    }
    let token = photos
        .token(&chosen, &Audience("webdav".into()))
        .await
        .expect("token");
    assert_eq!(token.value.expose(), "fake:fake-storage:webdav");

    let files = app(&service, "org.quire.Files");
    let other = files
        .find(&need, DataClass::Photos, Usage::Interactive)
        .await
        .expect("find");
    assert!(
        matches!(other, Found::NeedsConsent(_)),
        "another app's grant is not shared: {other:?}"
    );
}

#[tokio::test]
async fn a_refusal_sticks_for_that_app() {
    let service = Arc::new(fake_service(ScriptedPrompter::answering([Scripted::Deny])).await);
    let photos = app(&service, "org.quire.Photos");
    let need = storage(Delta::Poll);
    let offer = needs_consent(
        photos
            .find(&need, DataClass::Photos, Usage::Interactive)
            .await
            .expect("find"),
    );
    let refused = photos
        .request_grant(&offer, &ParentWindow::Unparented)
        .await;
    assert_eq!(refused, Err(ClientError::Refused(Refusal::Denied)));
    let after = photos
        .find(&need, DataClass::Photos, Usage::Interactive)
        .await
        .expect("find");
    assert_eq!(after, Found::None(NoAccount::Denied));
}

#[tokio::test]
async fn a_need_no_provider_meets_is_unsupported() {
    let service = Arc::new(fake_service(ScriptedPrompter::default()).await);
    let photos = app(&service, "org.quire.Photos");
    let found = photos
        .find(&storage(Delta::Push), DataClass::Photos, Usage::Interactive)
        .await
        .expect("find");
    assert_eq!(found, Found::None(NoAccount::Unsupported));
}

#[tokio::test]
async fn a_once_grant_is_spent_by_its_token() {
    let service = Arc::new(
        fake_service(ScriptedPrompter::answering([Scripted::AllowFirst(
            GrantScope::Once,
        )]))
        .await,
    );
    let photos = app(&service, "org.quire.Photos");
    let need = storage(Delta::Poll);
    let offer = needs_consent(
        photos
            .find(&need, DataClass::Files, Usage::Interactive)
            .await
            .expect("find"),
    );
    let chosen = photos
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");
    photos
        .token(&chosen, &Audience("webdav".into()))
        .await
        .expect("first token");
    let again = photos.token(&chosen, &Audience("webdav".into())).await;
    assert_eq!(again, Err(ClientError::Refused(Refusal::UnknownGrant)));
    let after = photos
        .find(&need, DataClass::Files, Usage::Interactive)
        .await
        .expect("find");
    assert!(matches!(after, Found::NeedsConsent(_)), "{after:?}");
}

#[tokio::test]
async fn revoking_a_grant_ends_it() {
    let service = Arc::new(
        fake_service(ScriptedPrompter::answering([Scripted::AllowFirst(
            GrantScope::Always,
        )]))
        .await,
    );
    let photos = app(&service, "org.quire.Photos");
    let need = storage(Delta::Poll);
    let offer = needs_consent(
        photos
            .find(&need, DataClass::Photos, Usage::Interactive)
            .await
            .expect("find"),
    );
    let chosen = photos
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");
    assert_eq!(photos.grants().await.expect("grants").len(), 1);
    photos.revoke(&chosen.grant).await.expect("revoke");
    assert_eq!(photos.grants().await.expect("grants"), vec![]);
}

#[tokio::test]
async fn no_family_signs_in_yet_so_adding_and_reauthenticating_are_unavailable_not_a_panic() {
    let prompter = ScriptedPrompter::answering([Scripted::AllowFirst(GrantScope::Always)]);
    let service = Arc::new(fake_service(prompter).await);
    let photos = app(&service, "org.quire.Photos");
    let offer = needs_consent(
        photos
            .find(&storage(Delta::Poll), DataClass::Photos, Usage::Interactive)
            .await
            .expect("find"),
    );
    let chosen = photos
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");

    assert_eq!(
        photos
            .add_account(
                porter_core::wire::ProviderHint::Any,
                &ParentWindow::Unparented
            )
            .await,
        Err(ClientError::Refused(Refusal::Unavailable))
    );
    assert_eq!(
        photos
            .reauthenticate(&chosen.account, &ParentWindow::Unparented)
            .await,
        Err(ClientError::Refused(Refusal::Unavailable))
    );
}
