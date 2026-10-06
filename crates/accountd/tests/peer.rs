//! `Accounts1.Peer`: callable only by a porter daemon.

mod common;

use common::*;
use porter_dbus::{CallerRole, PeerProxy};

fn photos_arg() -> (String, String) {
    ("org.quire.Photos".to_owned(), "flatpak".to_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_porter_daemon_may_ask_and_it_sees_the_consent_stores_verdict() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let inferd = rig
        .client_as(caller("org.quire.Inference", CallerRole::PorterDaemon))
        .await;
    let peer = PeerProxy::new(&inferd).await.expect("proxy");

    let before = peer
        .verdicts(&photos_arg(), &storage_need(), "photos", "interactive")
        .await
        .expect("verdicts");
    assert_eq!(before.len(), 1);
    assert_eq!(
        (before[0].0.as_str(), before[0].1.as_str()),
        ("fake-storage", "ask")
    );
    // Every row names the provider file its account was made from, granted or not.
    let provider = rig
        .service
        .registry()
        .accounts
        .iter()
        .find(|a| a.id.as_str() == "fake-storage")
        .map(|a| a.provider.to_string())
        .expect("the account");
    assert_eq!(text_of(&before[0].2, "provider"), Some(provider));

    let (_photos, grant) = grant_photos(&rig).await;
    let after = peer
        .verdicts(&photos_arg(), &storage_need(), "photos", "interactive")
        .await
        .expect("verdicts");
    assert_eq!(after[0].1, "granted");
    assert_eq!(text_of(&after[0].2, "grant"), Some(grant.to_string()));
    assert_eq!(text_of(&after[0].2, "scope").as_deref(), Some("always"));

    // The app is named by the daemon, so another app has no grant.
    let other = peer
        .verdicts(
            &("org.quire.Mail".to_owned(), "flatpak".to_owned()),
            &storage_need(),
            "photos",
            "interactive",
        )
        .await
        .expect("verdicts");
    assert_eq!(other[0].1, "ask");

    for refused in [
        rig.client_as(caller("org.example.App", CallerRole::App))
            .await,
        rig.client_as(caller("org.quire.Companion", CallerRole::Agent))
            .await,
        rig.client_as(caller("org.quire.Settings", CallerRole::Settings))
            .await,
        rig.stranger().await,
    ] {
        let err = PeerProxy::new(&refused)
            .await
            .expect("proxy")
            .verdicts(&photos_arg(), &storage_need(), "photos", "interactive")
            .await
            .expect_err("refused");
        assert_eq!(error_name(&err), ACCESS_DENIED);
    }
}
