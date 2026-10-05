//! The `Account` properties: readable only with a grant for the account.

mod common;

use common::*;
use porter_dbus::{AccountProxy, account_path};
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
async fn a_grant_holder_reads_the_account_and_only_the_kinds_it_was_granted() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let (photos_conn, _grant) = grant_photos(&rig).await;
    let storage = proxy_of(&photos_conn, &storage_account().id).await;
    assert_eq!(storage.id().await.expect("id"), "fake-storage");
    assert_eq!(storage.provider().await.expect("provider"), "fake-cloud");
    assert_eq!(storage.label().await.expect("label"), "ada@cloud.invalid");
    assert_eq!(storage.state().await.expect("state"), "ok");
    let kinds: Vec<String> = storage
        .capabilities()
        .await
        .expect("capabilities")
        .into_iter()
        .map(|(kind, _)| kind)
        .collect();
    assert_eq!(
        kinds,
        vec!["storage".to_owned()],
        "calendar was not granted"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_grant_the_account_does_not_exist_for_the_caller() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let _ = grant_photos(&rig).await;
    let mail_app = rig.client("org.quire.Mail").await;
    let storage = proxy_of(&mail_app, &storage_account().id).await;
    let id = storage.id().await.expect_err("no grant");
    assert!(error_name(&id).contains("UnknownObject"), "{id:?}");
    let capabilities = storage.capabilities().await.expect_err("no grant");
    assert!(
        error_name(&capabilities).contains("UnknownObject"),
        "{capabilities:?}"
    );

    let stranger = rig.stranger().await;
    let storage = proxy_of(&stranger, &storage_account().id).await;
    let id = storage.label().await.expect_err("unknown");
    assert!(error_name(&id).contains("AccessDenied"), "{id:?}");
}
