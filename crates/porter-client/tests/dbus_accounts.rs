//! accountd's immediate calls over a private bus, against the real `AccountService` core behind
//! a thin bus adapter: find, token, grants and revoke through `DbusTransport`, with refusals
//! arriving as `org.quire.Accounts1.Error.*` and read back as `Refusal`s.
#![cfg(feature = "dbus")]

mod common;

use common::accountd::Core;
use common::bus::PrivateBus;
use porter_client::{
    Accounts, ClientError, DbusTransport, Found, InProcess, NoAccount, TransportError,
};
use porter_core::capability::{Access, Delta, QuotaReport, StorageScope};
use porter_core::consent::{GrantScope, Usage};
use porter_core::need::StorageNeed;
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{AppId, AppName, Audience, DataClass, GrantId, Isolation, Need};
use porter_fake::{Scripted, ScriptedPrompter, fake_service};
use std::sync::Arc;

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Photos").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

fn storage(delta: Delta) -> Need {
    Need::Storage(StorageNeed {
        access: Access::ReadWrite,
        delta,
        scope: StorageScope::AppFolder,
        quota: QuotaReport::Unreported,
    })
}

struct Rig {
    _bus: PrivateBus,
    _daemon: zbus::Connection,
    /// The same service in process: the sheet (`Choose`) is not served over the bus yet, so a
    /// test consents here and then reads everything else over the bus.
    local: Accounts<
        InProcess<
            porter_fake::FakeProvider,
            porter_secrets::MemorySecrets,
            ScriptedPrompter,
            porter_fake::FixedClock,
        >,
    >,
    remote: Accounts<DbusTransport>,
}

async fn rig(script: impl IntoIterator<Item = Scripted>) -> Rig {
    let bus = PrivateBus::start();
    let service = Arc::new(fake_service(ScriptedPrompter::answering(script)).await);
    let daemon = bus.connect().await;
    Core {
        service: Arc::clone(&service),
        app: app(),
    }
    .serve(&daemon)
    .await;
    Rig {
        local: Accounts::over(InProcess::new(service, app())),
        remote: Accounts::over(DbusTransport::over(bus.connect().await)),
        _daemon: daemon,
        _bus: bus,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn find_token_grants_and_revoke_work_over_the_bus() {
    let rig = rig([Scripted::AllowFirst(GrantScope::Always)]).await;
    let need = storage(Delta::Poll);

    let before = rig
        .remote
        .find(&need, DataClass::Photos, Usage::Interactive)
        .await
        .expect("find");
    let offer = match before {
        Found::NeedsConsent(offer) => offer,
        other => panic!("expected NeedsConsent, got {other:?}"),
    };
    let chosen = rig
        .local
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted in process");

    match rig
        .remote
        .find(&need, DataClass::Photos, Usage::Interactive)
        .await
        .expect("find")
    {
        Found::One(candidate) => assert_eq!(candidate, chosen),
        other => panic!("expected One, got {other:?}"),
    }
    let token = rig
        .remote
        .token(&chosen, &Audience("webdav".into()))
        .await
        .expect("token over the bus");
    assert_eq!(token.value.expose(), "fake:fake-storage:webdav");

    let grants = rig.remote.grants().await.expect("grants");
    assert_eq!(grants, rig.local.grants().await.expect("local grants"));
    assert_eq!(grants.len(), 1);

    rig.remote.revoke(&chosen.grant).await.expect("revoke");
    assert_eq!(rig.remote.grants().await.expect("grants"), vec![]);
    assert!(matches!(
        rig.remote
            .find(&need, DataClass::Photos, Usage::Interactive)
            .await
            .expect("find"),
        Found::NeedsConsent(_)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsupported_need_is_an_answer_not_an_error() {
    let rig = rig([]).await;
    let found = rig
        .remote
        .find(&storage(Delta::Push), DataClass::Photos, Usage::Interactive)
        .await
        .expect("find");
    assert_eq!(found, Found::None(NoAccount::Unsupported));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_crosses_as_its_error_name() {
    let rig = rig([Scripted::AllowFirst(GrantScope::Always)]).await;
    let unknown = GrantId::parse("nope").expect("id");
    assert_eq!(
        rig.remote.revoke(&unknown).await,
        Err(ClientError::Refused(Refusal::UnknownGrant))
    );
    let offer = match rig
        .remote
        .find(&storage(Delta::Poll), DataClass::Photos, Usage::Interactive)
        .await
        .expect("find")
    {
        Found::NeedsConsent(offer) => offer,
        other => panic!("{other:?}"),
    };
    let chosen = rig
        .local
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");
    rig.remote.revoke(&chosen.grant).await.expect("revoke");
    assert_eq!(
        rig.remote.token(&chosen, &Audience("webdav".into())).await,
        Err(ClientError::Refused(Refusal::UnknownGrant)),
        "a token for a revoked grant is refused"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sheet_calls_wait_for_accountd_and_say_so_rather_than_panic() {
    let rig = rig([]).await;
    let got = rig
        .remote
        .add_account(ProviderHint::Any, &ParentWindow::Unparented)
        .await;
    assert!(
        matches!(
            got,
            Err(ClientError::Transport(TransportError::Malformed(_)))
        ),
        "{got:?}"
    );
}
