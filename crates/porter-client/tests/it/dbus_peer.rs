//! The daemon-only surface through the client (`peer::PeerAccounts`), against the real accountd
//! front end on a private bus: a porter daemon reads the consent store's verdicts as typed rows,
//! and every other caller is `PeerError::Denied`. No dictionary is parsed here: that is what the
//! reader is for.
#![cfg(feature = "dbus")]

use crate::common;

use common::accountd::{Daemon, photos};
use porter_client::peer::{PeerAccounts, PeerError};
use porter_client::{Accounts, DbusTransport, Found};
use porter_core::capability::{Access, Delta, QuotaReport, StorageScope};
use porter_core::consent::{GrantScope, Usage, Verdict};
use porter_core::need::StorageNeed;
use porter_core::wire::ParentWindow;
use porter_core::{AppId, AppName, DataClass, Isolation, Need};
use porter_dbus::{Caller, CallerRole};
use porter_fake::Scripted;

fn storage() -> Need {
    Need::Storage(StorageNeed::new(
        Access::ReadWrite,
        Delta::Poll,
        StorageScope::AppFolder,
        QuotaReport::Unreported,
    ))
}

fn caller(name: &str, role: CallerRole) -> Caller {
    Caller {
        app: AppId {
            name: AppName::parse(name).expect("app name"),
            isolation: Isolation::Unsandboxed,
        },
        role,
    }
}

async fn peer_as(daemon: &Daemon, who: Caller) -> PeerAccounts {
    let connection = daemon.bus.connect().await;
    let unique = connection.unique_name().expect("unique name").to_string();
    daemon.callers.introduce_as(&unique, who);
    PeerAccounts::over(&connection)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_porter_daemon_reads_the_verdicts_as_typed_rows_and_no_one_else_may() {
    let daemon = Daemon::start([Scripted::AllowFirst(GrantScope::Always)]).await;
    let inferd = peer_as(
        &daemon,
        caller("org.quire.Inference", CallerRole::PorterDaemon),
    )
    .await;
    let app = photos();

    let before = inferd
        .verdicts(&app, &storage(), DataClass::Photos, Usage::Interactive)
        .await
        .expect("verdicts");
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].verdict, Verdict::Ask);

    let remote = Accounts::over(DbusTransport::over(daemon.client_as(photos()).await));
    let offer = match remote
        .find(&storage(), DataClass::Photos, Usage::Interactive)
        .await
        .expect("find")
    {
        Found::NeedsConsent(offer) => offer,
        other => panic!("expected NeedsConsent, got {other:?}"),
    };
    let chosen = remote
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("chosen");

    let after = inferd
        .verdicts(&app, &storage(), DataClass::Photos, Usage::Interactive)
        .await
        .expect("verdicts");
    assert_eq!(after[0].account, chosen.account);
    assert_eq!(
        after[0].verdict,
        Verdict::Granted {
            grant: chosen.grant,
            scope: GrantScope::Always
        }
    );

    for refused in [
        caller("org.example.App", CallerRole::App),
        caller("org.quire.Settings", CallerRole::Settings),
    ] {
        let peer = peer_as(&daemon, refused).await;
        let got = peer
            .verdicts(&app, &storage(), DataClass::Photos, Usage::Interactive)
            .await;
        assert!(matches!(got, Err(PeerError::Denied(_))), "{got:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_accountd_the_daemon_is_told_it_is_not_there() {
    let bus = common::bus::PrivateBus::start();
    let connection = bus.connect().await;
    let got = PeerAccounts::over(&connection)
        .verdicts(&photos(), &storage(), DataClass::Photos, Usage::Interactive)
        .await;
    assert_eq!(got, Err(PeerError::Unreachable));
}
