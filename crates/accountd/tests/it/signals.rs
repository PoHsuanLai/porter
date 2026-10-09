//! The manager's signals are unicast: only the holders of a relevant grant hear them.

use crate::common;

use common::*;
use ds_settings::schema::KeyPath;
use porter_dbus::ManagerProxy;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn removing_an_account_tells_its_grant_holders_and_nobody_else() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let (photos_conn, _) = grant_photos(&rig).await;
    // Another app that has called accountd, but holds no grant.
    let mail_app = rig.client("org.quire.Mail").await;
    ManagerProxy::new(&mail_app)
        .await
        .expect("proxy")
        .query(&storage_need(), "photos", "interactive")
        .await
        .expect("query");
    let mut photos_hears = listen(&photos_conn).await;
    let mut mail_hears = listen(&mail_app).await;

    settings(&rig)
        .await
        .invoke(&KeyPath("accounts.fake-storage.remove".into()))
        .await
        .expect("removed");

    let photos = heard_names(&mut photos_hears, &["AccountRemoved", "GrantChanged"]).await;
    assert!(photos.contains(&"AccountRemoved".to_owned()), "{photos:?}");
    assert!(photos.contains(&"GrantChanged".to_owned()), "{photos:?}");
    // The holder has heard; that nobody else does is a short window by design (it proves an
    // absence, so a slow machine can only make it pass, never fail).
    let mail = heard(&mut mail_hears, Duration::from_millis(300)).await;
    assert!(mail.is_empty(), "{mail:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_toggle_is_a_capability_change_for_holders_only() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let (photos_conn, _) = grant_photos(&rig).await;
    let mail_app = rig.client("org.quire.Mail").await;
    ManagerProxy::new(&mail_app)
        .await
        .expect("proxy")
        .query(&storage_need(), "photos", "interactive")
        .await
        .expect("query");
    let mut photos_hears = listen(&photos_conn).await;
    let mut mail_hears = listen(&mail_app).await;

    settings(&rig)
        .await
        .set(
            &KeyPath("accounts.fake-storage.service.calendar".into()),
            &toml::Value::String("off".into()),
        )
        .await
        .expect("toggled");

    let photos = heard_names(&mut photos_hears, &["CapabilityChanged"]).await;
    assert_eq!(photos, vec!["CapabilityChanged".to_owned()]);
    assert!(
        heard(&mut mail_hears, Duration::from_millis(300))
            .await
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_grant_is_announced_to_its_holder_alone() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let photos_conn = rig.client("org.quire.Photos").await;
    let mail_app = rig.client("org.quire.Mail").await;
    ManagerProxy::new(&mail_app)
        .await
        .expect("proxy")
        .query(&storage_need(), "photos", "interactive")
        .await
        .expect("query");
    let mut photos_hears = listen(&photos_conn).await;
    let mut mail_hears = listen(&mail_app).await;
    let (code, _) = choose(&photos_conn).await;
    assert_eq!(code, 0);
    let photos = heard_names(&mut photos_hears, &["GrantChanged"]).await;
    assert_eq!(photos, vec!["GrantChanged".to_owned()]);
    assert!(
        heard(&mut mail_hears, Duration::from_millis(300))
            .await
            .is_empty()
    );
}
