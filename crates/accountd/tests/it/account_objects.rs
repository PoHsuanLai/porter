//! The `Account` properties: readable only by the shell and Settings. No app holds a grant for an
//! account any more, so for every other caller an account does not exist.

use crate::common;

use common::*;
use porter_callers::CallerRole;
use porter_dbus::accounts::AccountProxy;
use porter_dbus::accounts::account_path;
use zbus::zvariant::ObjectPath;

async fn proxy_of(
    connection: &zbus::Connection,
    id: &porter_core::AccountId,
) -> AccountProxy<'static> {
    AccountProxy::builder(connection)
        .path(
            ObjectPath::try_from(account_path(id))
                .expect("path")
                .into_owned(),
        )
        .expect("path")
        .build()
        .await
        .expect("proxy")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shell_and_settings_read_the_account_and_every_kind_it_offers() {
    let rig = Rig::start().await;
    for who in [
        caller("org.example.SheetHost", CallerRole::SheetHost),
        caller("org.quire.Settings", CallerRole::Settings),
    ] {
        let connection = rig.client_as(who).await;
        let storage = proxy_of(&connection, &storage_account().id).await;
        assert_eq!(storage.id().await.expect("id"), "fake-storage");
        assert_eq!(storage.provider().await.expect("provider"), "fake-cloud");
        assert_eq!(storage.label().await.expect("label"), "ada@cloud.invalid");
        assert_eq!(storage.state().await.expect("state"), "ok");
        let mut kinds: Vec<String> = storage
            .capabilities()
            .await
            .expect("capabilities")
            .into_iter()
            .map(|(kind, _)| kind)
            .collect();
        kinds.sort();
        kinds.dedup();
        assert_eq!(kinds, vec!["calendar".to_owned(), "storage".to_owned()]);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn for_an_app_the_account_does_not_exist_and_a_stranger_is_denied() {
    let rig = Rig::start().await;
    for who in [
        caller("org.quire.Mail", CallerRole::App),
        caller("org.quire.Inference", CallerRole::PorterDaemon),
    ] {
        let app = rig.client_as(who).await;
        let storage = proxy_of(&app, &storage_account().id).await;
        let id = storage.id().await.expect_err("an app reads no account");
        assert!(error_name(&id).contains("UnknownObject"), "{id:?}");
        let capabilities = storage.capabilities().await.expect_err("no account");
        assert!(
            error_name(&capabilities).contains("UnknownObject"),
            "{capabilities:?}"
        );
    }

    let stranger = rig.stranger().await;
    let storage = proxy_of(&stranger, &storage_account().id).await;
    let id = storage.label().await.expect_err("unknown");
    assert!(error_name(&id).contains("AccessDenied"), "{id:?}");
}
