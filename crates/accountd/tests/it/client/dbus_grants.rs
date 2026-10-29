//! `Grants` against the real accountd (`common::accountd`, over the fake providers and a scripted
//! consent sheet): finding, asking, listing, revoking and opening a relayed stream over
//! `porter-client-dbus`'s `Bus`. The consent sheet shows once and its answer is kept; a refusal
//! sticks; a stream arrives as a descriptor; and only a porter daemon lists the accounts.

use super::common;

use common::accountd::{Daemon, named};
use porter_callers::{Caller, CallerRole};
use porter_capability::{Access, CapabilityKind, Delta, StorageScope};
use porter_client::{
    AuthenticatedStream, ConsentOffer, Found, Grants, GrantsError, LinkError, NoAccount, Porter,
};
use porter_client_dbus::Bus;
use porter_core::consent::{GrantScope, Usage};
use porter_core::{AccountId, DataClass, ProviderId};
use porter_fake::Scripted;
use porter_match::{Need, StorageNeed};
use porter_wire::{ParentWindow, Refusal};

fn need() -> Need {
    Need::Storage(StorageNeed::new(
        Access::ReadWrite,
        Delta::Poll,
        StorageScope::AppFolder,
        false,
    ))
}

fn offer() -> ConsentOffer {
    ConsentOffer {
        need: need(),
        class: DataClass::Photos,
        usage: Usage::Interactive,
        account: None,
    }
}

async fn grants(daemon: &Daemon) -> Grants<Bus> {
    Porter::over(Bus::over(daemon.client().await)).grants()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_is_asked_for_once_found_again_opened_as_a_descriptor_and_revoked() {
    let daemon = Daemon::start([Scripted::AllowFirst(GrantScope::Always)]).await;
    let grants = grants(&daemon).await;

    assert!(matches!(
        grants
            .find(&need(), DataClass::Photos, Usage::Interactive)
            .await,
        Ok(Found::NeedsConsent(_))
    ));
    let chosen = grants
        .request_grant(&offer(), &ParentWindow::Unparented)
        .await
        .expect("granted");
    assert_eq!(
        chosen.account,
        AccountId::parse("fake-storage").expect("id")
    );
    assert_eq!(daemon.asked.asked().len(), 1);

    // The answer is kept: the app finds the grant and is asked no more.
    match grants
        .find(&need(), DataClass::Photos, Usage::Interactive)
        .await
        .expect("find")
    {
        Found::One(candidate) => assert_eq!(candidate, chosen),
        other => panic!("expected One, got {other:?}"),
    }
    assert_eq!(daemon.asked.asked().len(), 1);
    let held = grants.grants().await.expect("grants");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].id, chosen.grant);

    // The stream arrives as a descriptor; where it leads is accountd's relay's business.
    let stream = grants
        .open_authenticated(&chosen.grant, &chosen.endpoints[0].url)
        .await
        .expect("a stream");
    assert!(matches!(stream, AuthenticatedStream::Fd(_)), "{stream:?}");

    grants.revoke(&chosen.grant).await.expect("revoked");
    assert!(matches!(
        grants
            .find(&need(), DataClass::Photos, Usage::Interactive)
            .await,
        Ok(Found::NeedsConsent(_))
    ));
    assert_eq!(
        grants.revoke(&chosen.grant).await,
        Err(GrantsError::Refused(Refusal::UnknownGrant))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_sticks_and_says_so() {
    let daemon = Daemon::start([Scripted::Deny]).await;
    let grants = grants(&daemon).await;
    assert_eq!(
        grants
            .request_grant(&offer(), &ParentWindow::Unparented)
            .await,
        Err(GrantsError::Refused(Refusal::Denied))
    );
    assert_eq!(
        grants
            .find(&need(), DataClass::Photos, Usage::Interactive)
            .await,
        Ok(Found::None(NoAccount::Denied))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_offer_that_names_an_account_asks_about_that_account_alone() {
    let daemon = Daemon::start([Scripted::AllowFirst(GrantScope::Always)]).await;
    let grants = grants(&daemon).await;

    // No such account fits: nothing is asked and nothing is granted.
    let elsewhere = ConsentOffer {
        account: Some(AccountId::parse("somewhere-else").expect("id")),
        ..offer()
    };
    assert_eq!(
        grants
            .request_grant(&elsewhere, &ParentWindow::Unparented)
            .await,
        Err(GrantsError::Refused(Refusal::NoFittingAccount))
    );
    assert!(grants.grants().await.expect("grants").is_empty());

    let named = ConsentOffer {
        account: Some(AccountId::parse("fake-storage").expect("id")),
        ..offer()
    };
    let chosen = grants
        .request_grant(&named, &ParentWindow::Unparented)
        .await
        .expect("granted");
    assert_eq!(
        chosen.account,
        AccountId::parse("fake-storage").expect("id")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_porter_daemon_lists_the_accounts_and_an_app_or_a_stranger_may_not() {
    let daemon = Daemon::start([]).await;
    let connection = daemon.bus.connect().await;
    daemon.callers.introduce_as(
        connection.unique_name().expect("unique name").as_str(),
        Caller {
            app: named("org.quire.Sync"),
            role: CallerRole::PorterDaemon,
        },
    );
    let listed = Porter::over(Bus::over(connection))
        .grants()
        .accounts()
        .await
        .expect("accounts");
    let ids: Vec<&str> = listed.iter().map(|row| row.account.id.as_str()).collect();
    assert!(ids.contains(&"fake-storage"), "{ids:?}");

    assert_eq!(
        grants(&daemon).await.accounts().await,
        Err(GrantsError::Link(LinkError::Denied))
    );
    let stranger = Porter::over(Bus::over(daemon.stranger().await)).grants();
    assert_eq!(
        stranger
            .find(&need(), DataClass::Photos, Usage::Interactive)
            .await,
        Err(GrantsError::Link(LinkError::Denied))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn extending_access_and_asking_whether_a_provider_does_are_calls_of_the_grants_domain() {
    let daemon = Daemon::start([]).await;
    let connection = daemon.bus.connect().await;
    daemon.callers.introduce(
        connection.unique_name().expect("unique name").as_str(),
        named("org.quire.Sync"),
    );
    let grants = Porter::over(Bus::over(connection)).grants();
    assert_eq!(
        grants
            .extend_access(
                &AccountId::parse("nobody").expect("id"),
                &[CapabilityKind::Storage],
                &ParentWindow::Unparented,
            )
            .await,
        Err(GrantsError::Refused(Refusal::UnknownGrant))
    );
    assert_eq!(
        grants
            .asks_on_use(
                &ProviderId::parse("no-such-provider").expect("id"),
                CapabilityKind::Storage,
            )
            .await,
        Ok(false)
    );
}
