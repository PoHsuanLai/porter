//! The bugs the jailed accounts scenarios found, each on a private bus: the shell and Settings
//! sign any account in again (an app cannot), and the settings module says when its rows came or
//! went.

use crate::common;

use common::*;
use ds_settings::live::Changes;
use porter_dbus::Details;
use porter_dbus::accounts::{AccountProxy, Sheet};
use porter_dbus::callers::CallerRole;
use porter_sheet::SheetInput;
use std::time::Duration;
use zbus::export::futures_core::Stream;
use zbus::zvariant::ObjectPath;

/// The settings module's next `Changed`, or none within `within`.
async fn next_change(changes: &mut Changes, within: Duration) -> Option<(String, toml::Value)> {
    tokio::time::timeout(
        within,
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut *changes).poll_next(cx)),
    )
    .await
    .ok()
    .flatten()
    .and_then(Result::ok)
    .map(|change| (change.key.0, change.value))
}

/// Every `Changed` the module sends until it has sent `wanted`.
async fn changes_until(
    changes: &mut Changes,
    wanted: (&str, toml::Value),
) -> Vec<(String, toml::Value)> {
    let wanted = (wanted.0.to_owned(), wanted.1);
    let mut seen = Vec::new();
    while !seen.contains(&wanted) {
        match next_change(changes, test_kit::GENEROUS).await {
            Some(change) => seen.push(change),
            None => panic!("never happened: the change {wanted:?}: {seen:?}"),
        }
    }
    seen
}

fn said(text: &str) -> toml::Value {
    toml::Value::String(text.to_owned())
}

/// A host that answers every sheet with a dismissal, so a sign-in that starts ends at once.
fn dismissing() -> SheetHost {
    SheetHost::answering(SheetInput::Dismiss)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sheet_host_signs_any_account_in_again() {
    let rig = Rig::start_with(Default::default(), dismissing()).await;
    let shell = rig
        .client_as(caller("org.quire.Shell", CallerRole::SheetHost))
        .await;
    let account = AccountProxy::builder(&shell)
        .path(
            ObjectPath::try_from(porter_dbus::accounts::account_path(&mail_account().id))
                .expect("path"),
        )
        .expect("path")
        .build()
        .await
        .expect("account");
    let mut sheet = Sheet::subscribe(&shell).await.expect("subscribe");
    let request = account
        .reauthenticate("", &sheet.options())
        .await
        .expect("a request");
    let (_code, results) = sheet.response(&request).await.expect("response");
    // The sheet opened: the host was asked.
    assert_eq!(rig.host_log.calls().opened.len(), 1, "{results:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_signs_any_account_in_again() {
    let rig = Rig::start_with(Default::default(), dismissing()).await;
    settings(&rig)
        .await
        .invoke(&key("accounts.fake-mail.reauth"))
        .await
        .expect("started");
    eventually("the sign-in sheet to open", || {
        rig.host_log.calls().opened.len() == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_cannot_sign_an_account_in_again() {
    let rig = Rig::start_with(Default::default(), dismissing()).await;
    let app = rig.client("org.quire.Mail").await;
    let account = AccountProxy::builder(&app)
        .path(
            ObjectPath::try_from(porter_dbus::accounts::account_path(&mail_account().id))
                .expect("path"),
        )
        .expect("path")
        .build()
        .await
        .expect("account");
    let error = account
        .reauthenticate("", &Details::new())
        .await
        .expect_err("an app does not see the account");
    assert_eq!(error_name(&error), UNKNOWN_OBJECT);
    assert!(rig.host_log.calls().opened.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_settings_listener_hears_an_account_go() {
    let rig = Rig::start().await;
    let client = settings(&rig).await;
    let mut changes = client.changes().await.expect("changes");

    client
        .invoke(&key("accounts.fake-mail.remove"))
        .await
        .expect("removed");
    let told = changes_until(&mut changes, ("accounts.fake-mail.state", said("removed"))).await;
    assert!(
        told.contains(&("accounts.fake-mail.state".to_owned(), said("removed"))),
        "{told:?}"
    );
    assert!(
        !client
            .describe()
            .await
            .expect("schema")
            .key
            .iter()
            .any(|k| k.path.0.starts_with("accounts.fake-mail.")),
        "the schema a pane reads again no longer lists it"
    );
}
