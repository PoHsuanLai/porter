//! The manager's signals are unicast: only the shell and Settings, the two roles that read an
//! account, hear them. No app does.

use crate::common;

use common::*;
use ds_settings::schema::KeyPath;
use porter_core::AccountState;
use porter_dbus::{AccountProxy, CallerRole, account_path};
use std::time::Duration;
use zbus::zvariant::ObjectPath;

/// A connection that has called accountd as `who`, so that it is among those told.
async fn joined(rig: &Rig, who: porter_dbus::Caller) -> zbus::Connection {
    let connection = rig.client_as(who).await;
    let account = AccountProxy::builder(&connection)
        .path(ObjectPath::try_from(account_path(&storage_account().id)).expect("path"))
        .expect("path")
        .build()
        .await
        .expect("proxy");
    // An app is refused here; the call still puts it on the roster.
    let _ = account.id().await;
    connection
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_an_account_tells_the_shell_and_settings_and_no_app() {
    let rig = Rig::start().await;
    let shell = joined(
        &rig,
        caller("org.example.SheetHost", CallerRole::SheetHost),
    )
    .await;
    let settings_conn = joined(&rig, caller("org.quire.Settings", CallerRole::Settings)).await;
    let app = joined(&rig, caller("org.quire.Mail", CallerRole::App)).await;
    let mut shell_hears = listen(&shell).await;
    let mut settings_hears = listen(&settings_conn).await;
    let mut app_hears = listen(&app).await;

    settings(&rig)
        .await
        .invoke(&KeyPath("accounts.fake-storage.remove".into()))
        .await
        .expect("removed");

    for hears in [&mut shell_hears, &mut settings_hears] {
        let told = heard_names(hears, &["AccountRemoved"]).await;
        assert!(told.contains(&"AccountRemoved".to_owned()), "{told:?}");
    }
    // That no app hears is a short window by design (it proves an absence, so a slow machine can
    // only make it pass, never fail).
    let apps = heard(&mut app_hears, Duration::from_millis(300)).await;
    assert!(apps.is_empty(), "{apps:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_that_needs_signing_in_again_is_told_to_the_shell_alone_among_apps() {
    let rig = Rig::start().await;
    let shell = joined(
        &rig,
        caller("org.example.SheetHost", CallerRole::SheetHost),
    )
    .await;
    let app = joined(&rig, caller("org.quire.Mail", CallerRole::App)).await;
    let mut shell_hears = listen(&shell).await;
    let mut app_hears = listen(&app).await;

    assert!(
        rig.service
            .set_state(&storage_account().id, AccountState::NeedsReauth)
            .await
    );
    // The service changed under the objects; the next call publishes it.
    let _ = settings(&rig).await.describe().await;
    let _ = rig
        .client_as(caller("org.quire.Settings", CallerRole::Settings))
        .await;
    settings(&rig)
        .await
        .set(
            &KeyPath("accounts.fake-mail.reauth".into()),
            &toml::Value::Boolean(true),
        )
        .await
        .ok();

    let told = heard_names(&mut shell_hears, &["NeedsReauth"]).await;
    assert!(told.contains(&"NeedsReauth".to_owned()), "{told:?}");
    let apps = heard(&mut app_hears, Duration::from_millis(300)).await;
    assert!(apps.is_empty(), "{apps:?}");
}
