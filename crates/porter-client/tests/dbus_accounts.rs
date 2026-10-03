//! accountd's calls over a private bus, against the real accountd front end (`accountd::serve`)
//! over the real `AccountService` core: find, choose, token, grants and revoke through
//! `DbusTransport`, with refusals arriving as `org.quire.Accounts1.Error.*` (the immediate
//! calls) or as a `Response` (the sheets) and read back as `Refusal`s. The sheets' own contract
//! (paths, codes, ordering, close) is in `dbus_sheets.rs`.
#![cfg(feature = "dbus")]

mod common;

use common::accountd::{Daemon, photos};
use porter_client::{Accounts, ClientError, DbusTransport, Found, NoAccount};
use porter_core::capability::{Access, Delta, QuotaReport, StorageScope};
use porter_core::consent::{GrantScope, Usage};
use porter_core::need::StorageNeed;
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{Audience, DataClass, GrantId, Need};
use porter_fake::Scripted;

fn storage(delta: Delta) -> Need {
    Need::Storage(StorageNeed {
        access: Access::ReadWrite,
        delta,
        scope: StorageScope::AppFolder,
        quota: QuotaReport::Unreported,
    })
}

struct Rig {
    daemon: Daemon,
    remote: Accounts<DbusTransport>,
}

async fn rig(script: impl IntoIterator<Item = Scripted>) -> Rig {
    let daemon = Daemon::start(script).await;
    let remote = Accounts::over(DbusTransport::over(daemon.client_as(photos()).await));
    Rig { daemon, remote }
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
        .remote
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("chosen over the bus");

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
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].id, chosen.grant);
    assert_eq!(rig.daemon.service.registry().grants, grants);

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
        .remote
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
async fn an_unknown_caller_is_refused_by_the_bus_not_answered() {
    let daemon = Daemon::start([]).await;
    let stranger = Accounts::over(DbusTransport::over(daemon.stranger().await));
    let got = stranger
        .find(&storage(Delta::Poll), DataClass::Photos, Usage::Interactive)
        .await;
    assert!(
        matches!(&got, Err(ClientError::Transport(porter_client::TransportError::Malformed(why))) if why.contains("refused by the bus")),
        "{got:?}"
    );
}
