//! The sheet methods through `DbusTransport` against the real accountd: `Choose`, `AddAccount`
//! and `Reauthenticate` return a Request object, and the answer is its `Response` signal (code 0
//! the answer, 1 closed, 2 another refusal). A client that gives up closes the sheet; one whose
//! daemon leaves is told the link closed.
#![cfg(feature = "dbus")]

use crate::common;

use common::accountd::{Daemon, named, photos};
use common::eventually;
use porter_client::{Accounts, ClientError, DbusTransport, Found, TransportError};
use porter_core::capability::{Access, Delta, QuotaReport, StorageScope};
use porter_core::consent::{GrantScope, Usage};
use porter_core::need::StorageNeed;
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{AccountId, Audience, DataClass, Need};
use porter_fake::Scripted;
use std::sync::Arc;
use std::time::Duration;

fn storage(delta: Delta) -> Need {
    Need::Storage(StorageNeed {
        access: Access::ReadWrite,
        delta,
        scope: StorageScope::AppFolder,
        quota: QuotaReport::Unreported,
    })
}

async fn offer_for(
    accounts: &Accounts<DbusTransport>,
    delta: Delta,
) -> porter_client::ConsentOffer {
    match accounts
        .find(&storage(delta), DataClass::Photos, Usage::Interactive)
        .await
        .expect("find")
    {
        Found::NeedsConsent(offer) => offer,
        other => panic!("expected NeedsConsent, got {other:?}"),
    }
}

async fn app(daemon: &Daemon) -> Accounts<DbusTransport> {
    Accounts::over(DbusTransport::over(daemon.client().await))
}

#[tokio::test(flavor = "multi_thread")]
async fn choosing_over_the_bus_grants_and_returns_the_account_with_its_grant() {
    let daemon = Daemon::start([Scripted::AllowFirst(GrantScope::Always)]).await;
    let photos_app = app(&daemon).await;
    let offer = offer_for(&photos_app, Delta::Poll).await;

    let chosen = photos_app
        .request_grant(&offer, &ParentWindow::Handle("wayland:abc".into()))
        .await
        .expect("chosen");

    assert_eq!(chosen.account.as_str(), "fake-storage");
    let asks = daemon.asked.asked();
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0].app, photos());
    let found = photos_app
        .find(&storage(Delta::Poll), DataClass::Photos, Usage::Interactive)
        .await
        .expect("find");
    assert_eq!(found, Found::One(chosen.clone()));
    let token = photos_app
        .token(&chosen, &Audience("webdav".into()))
        .await
        .expect("a token for the new grant");
    assert_eq!(token.value.expose(), "fake:fake-storage:webdav");
}

#[tokio::test(flavor = "multi_thread")]
async fn every_way_a_sheet_can_end_is_the_refusal_the_in_process_carrier_gives() {
    let cases = [
        ("closed", Scripted::Dismiss, Delta::Poll, Refusal::Dismissed),
        ("dont allow", Scripted::Deny, Delta::Poll, Refusal::Denied),
        (
            "no account fits",
            Scripted::Dismiss,
            Delta::Push,
            Refusal::NoFittingAccount,
        ),
    ];
    for (name, script, delta, refusal) in cases {
        let daemon = Daemon::start([script]).await;
        let photos_app = app(&daemon).await;
        let offer = porter_client::ConsentOffer {
            need: storage(delta),
            class: DataClass::Photos,
            usage: Usage::Interactive,
        };
        assert_eq!(
            photos_app
                .request_grant(&offer, &ParentWindow::Unparented)
                .await,
            Err(ClientError::Refused(refusal)),
            "{name}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_sheets_at_once_from_one_connection_each_get_their_own_answer() {
    let daemon = Daemon::start([Scripted::AllowFirst(GrantScope::Always), Scripted::Deny]).await;
    let photos_app = app(&daemon).await;
    let offer = offer_for(&photos_app, Delta::Poll).await;
    let window = ParentWindow::Unparented;
    let (one, two) = tokio::join!(
        photos_app.request_grant(&offer, &window),
        photos_app.request_grant(&offer, &window)
    );
    let mut answers = [
        one.map(|c| c.account.to_string()),
        two.map(|c| c.account.to_string()),
    ];
    answers.sort_by_key(|answer| answer.is_ok());
    assert_eq!(answers[0], Err(ClientError::Refused(Refusal::Denied)));
    assert_eq!(answers[1], Ok("fake-storage".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn adding_an_account_is_unavailable_until_a_family_signs_in_and_accountd_goes_on() {
    let daemon = Daemon::start([Scripted::AllowFirst(GrantScope::Always)]).await;
    let photos_app = app(&daemon).await;
    for hint in [
        ProviderHint::Any,
        ProviderHint::Provider("nextcloud".parse_provider()),
    ] {
        assert_eq!(
            photos_app
                .add_account(hint, &ParentWindow::Unparented)
                .await,
            Err(ClientError::Refused(Refusal::Unavailable))
        );
    }
    // The daemon is still serving.
    let offer = offer_for(&photos_app, Delta::Poll).await;
    assert!(
        photos_app
            .request_grant(&offer, &ParentWindow::Unparented)
            .await
            .is_ok()
    );
}

trait ParseProvider {
    fn parse_provider(&self) -> porter_core::ProviderId;
}

impl ParseProvider for str {
    fn parse_provider(&self) -> porter_core::ProviderId {
        porter_core::ProviderId::parse(self).expect("provider id")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reauthenticating_needs_a_grant_for_the_account_and_is_unavailable_without_a_family() {
    let daemon = Daemon::start([Scripted::AllowFirst(GrantScope::Always)]).await;
    let photos_app = app(&daemon).await;
    let storage_account = AccountId::parse("fake-storage").expect("id");

    // No grant: the account does not exist for this app (no bulk enumeration).
    let before = photos_app
        .reauthenticate(&storage_account, &ParentWindow::Unparented)
        .await;
    assert!(
        matches!(&before, Err(ClientError::Transport(TransportError::Malformed(why))) if why.contains("UnknownObject")),
        "{before:?}"
    );

    let offer = offer_for(&photos_app, Delta::Poll).await;
    photos_app
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");
    assert_eq!(
        photos_app
            .reauthenticate(&storage_account, &ParentWindow::Unparented)
            .await,
        Err(ClientError::Refused(Refusal::Unavailable))
    );

    // Another app holds no grant for it.
    let mail = Accounts::over(DbusTransport::over(
        daemon.client_as(named("org.quire.Mail")).await,
    ));
    assert!(matches!(
        mail.reauthenticate(&storage_account, &ParentWindow::Unparented)
            .await,
        Err(ClientError::Transport(TransportError::Malformed(_)))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_drops_the_call_takes_the_sheet_down() {
    let daemon = Daemon::start([Scripted::Hang]).await;
    let photos_app = Arc::new(app(&daemon).await);
    let offer = offer_for(&photos_app, Delta::Poll).await;
    let waiting = {
        let photos_app = Arc::clone(&photos_app);
        tokio::spawn(async move {
            photos_app
                .request_grant(&offer, &ParentWindow::Unparented)
                .await
        })
    };
    eventually("the sheet to be shown", || daemon.asked.asked().len() == 1).await;
    assert_eq!(daemon.asked.abandoned(), 0, "the sheet is open");

    waiting.abort();
    let _ = waiting.await;

    eventually("the sheet to be closed", || daemon.asked.abandoned() == 1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_leaves_mid_sheet_ends_the_wait_with_closed() {
    let daemon = Daemon::start([Scripted::Hang]).await;
    let photos_app = Arc::new(app(&daemon).await);
    let offer = offer_for(&photos_app, Delta::Poll).await;
    let waiting = {
        let photos_app = Arc::clone(&photos_app);
        tokio::spawn(async move {
            photos_app
                .request_grant(&offer, &ParentWindow::Unparented)
                .await
        })
    };
    eventually("the sheet to be shown", || daemon.asked.asked().len() == 1).await;

    daemon.connection.close().await.expect("the daemon leaves");

    let ended = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("the wait ends")
        .expect("task");
    assert_eq!(ended, Err(ClientError::Transport(TransportError::Closed)));
}

mod racing {
    use super::*;
    use crate::common::bus::PrivateBus;
    use crate::common::racing::{Racing, candidate};

    async fn racing_daemon(
        bus: &PrivateBus,
        stranger: Option<zbus::Connection>,
    ) -> zbus::Connection {
        let daemon = bus.connect().await;
        daemon
            .object_server()
            .at(porter_dbus::ACCOUNTS_PATH, Racing { stranger })
            .await
            .expect("serve");
        daemon
            .request_name(porter_dbus::ACCOUNTS_BUS)
            .await
            .expect("name");
        daemon
    }

    fn offer() -> porter_client::ConsentOffer {
        porter_client::ConsentOffer {
            need: storage(Delta::Poll),
            class: DataClass::Photos,
            usage: Usage::Interactive,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_response_sent_before_the_reply_is_not_lost() {
        let bus = PrivateBus::start();
        let _daemon = racing_daemon(&bus, None).await;
        let client = Accounts::over(DbusTransport::over(bus.connect().await));
        let chosen = client
            .request_grant(&offer(), &ParentWindow::Unparented)
            .await
            .expect("the early response is read");
        assert_eq!(chosen, candidate());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_response_from_a_connection_that_does_not_own_the_name_is_ignored() {
        let bus = PrivateBus::start();
        let _daemon = racing_daemon(&bus, Some(bus.connect().await)).await;
        let client = Accounts::over(DbusTransport::over(bus.connect().await));
        let chosen = client
            .request_grant(&offer(), &ParentWindow::Unparented)
            .await
            .expect("the forged dismissal did not answer for the person");
        assert_eq!(chosen, candidate());
    }
}
