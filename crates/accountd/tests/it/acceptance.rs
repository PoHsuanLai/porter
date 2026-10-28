//! Acceptance 3 and 8 of design/31 §7.1 on a private bus with scratch HOME and XDG, over the
//! real `AccountService` with the fake providers and in-memory secrets. (Acceptance 1, an app
//! finding nothing until consent, is gone with the grants: an app learns of an identity only by
//! `Identities1`, which `identities.rs` covers.)
//!
//! - 3: no refresh token, password or key is in any message on the bus.
//! - 8: removal leaves nothing: secrets, registry row, signals, audit.

use crate::common;

use common::*;
use ds_settings::live::{AnyOwner, LiveClient};
use ds_settings::schema::KeyPath;
use porter_callers::CallerRole;
use porter_core::{SecretKey, SecretPurpose};
use porter_dbus::accounts::account_path;
use porter_dbus::accounts::{AccountProxy, IdentitiesProxy};
use porter_secrets::{Secrets, SecretsError};
use zbus::zvariant::ObjectPath;

/// The storage account's object, read live each time: a cached property would outlive the
/// object, and the tests ask whether the object is still there.
async fn account_proxy(connection: &zbus::Connection) -> AccountProxy<'_> {
    AccountProxy::builder(connection)
        .path(ObjectPath::try_from(account_path(&storage_account().id)).expect("path"))
        .expect("path")
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .expect("proxy")
}

#[tokio::test(flavor = "multi_thread")]
async fn acceptance_3_no_secret_is_in_any_message_on_the_bus() {
    let rig = Rig::start().await;
    let mut tap = Tap::start(&rig.bus).await;

    // Everything an app and the shell can ask, for the accounts that hold secrets.
    let app = rig.client("org.quire.Photos").await;
    let identities = IdentitiesProxy::new(&app).await.expect("proxy");
    identities.list().await.expect("list");
    let shell = rig
        .client_as(caller("org.example.SheetHost", CallerRole::SheetHost))
        .await;
    let account = account_proxy(&shell).await;
    let _ = (
        account.id().await,
        account.capabilities().await,
        account.state().await,
    );
    // Settings reads every key of every account, the mail account (OAuth) included.
    let settings = settings(&rig).await;
    for row in settings.describe().await.expect("schema").key {
        let _ = settings.get(&row.path).await;
    }
    let _ = settings.snapshot().await.expect("snapshot");

    let seen = tap.drain().await;
    assert!(
        seen.len() > 20,
        "the monitor saw the traffic: {}",
        seen.len()
    );
    for secret in [APP_PASSWORD, REFRESH_TOKEN, ACCESS_TOKEN] {
        assert!(
            !seen.iter().any(|message| contains(message, secret)),
            "{secret} crossed the bus"
        );
    }
    assert!(
        seen.iter().any(|m| contains(m, "ada@mail.invalid")),
        "positive control: the scan finds a value that did cross"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn acceptance_8_removal_leaves_nothing_and_the_reader_is_told() {
    let rig = Rig::start().await;
    let connection = rig
        .client_as(caller("org.quire.Settings", CallerRole::Settings))
        .await;
    // Reading an account puts the connection among those that are told.
    let account = account_proxy(&connection).await;
    account.id().await.expect("settings reads accounts");
    let mut hears = listen(&connection).await;
    let mail = SecretKey {
        account: mail_account().id,
        purpose: SecretPurpose::OAuthRefresh,
    };
    let password = SecretKey {
        account: storage_account().id,
        purpose: SecretPurpose::Password,
    };
    let settings = LiveClient::new(
        &connection,
        "org.quire.Accounts1",
        &accountd::settings_path(),
        AnyOwner,
    )
    .expect("client");
    settings
        .invoke(&KeyPath("accounts.fake-storage.remove".into()))
        .await
        .expect("remove");

    // Secrets: this account's are gone, the other's stay.
    assert_eq!(rig.secrets.get(&password).await, Err(SecretsError::Missing));
    assert!(rig.secrets.get(&mail).await.is_ok());
    // Registry, in memory and on the store.
    let registry = rig.service.registry();
    assert!(
        registry
            .accounts
            .iter()
            .all(|a| a.id != storage_account().id)
    );
    let stored = rig.store.stored().expect("stored");
    assert!(stored.accounts.iter().all(|a| a.id != storage_account().id));
    // The object is gone.
    assert!(account.id().await.is_err());
    // The reader was told, once.
    let told = heard_names(&mut hears, &["AccountRemoved"]).await;
    assert_eq!(
        told.iter().filter(|n| *n == "AccountRemoved").count(),
        1,
        "{told:?}"
    );
    // The audit names the removal.
    assert!(rig.audit.entries().iter().any(|e| e.event
        == porter_service_core::audit::AuditEvent::Removed
        && e.account == Some(storage_account().id)));
}
