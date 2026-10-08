//! `org.quire.SettingsModule1` on accountd: only the Settings role, the rows of PLAN §2.5, and
//! what each `Set` does to the registry.

use crate::common;

use common::*;
use ds_settings::live::LiveError;
use ds_settings::schema::KeyPath;
use porter_core::audit::AuditEvent;
use porter_core::{AccountState, CapabilityKind, SecretKey, SecretPurpose};
use porter_dbus::{CallerRole, ManagerProxy};
use porter_secrets::{Secrets, SecretsError};

#[tokio::test(flavor = "multi_thread")]
async fn only_the_settings_role_may_read_or_set_anything() {
    let rig = Rig::start().await;
    for who in [
        caller("org.example.App", CallerRole::App),
        caller("org.quire.Companion", CallerRole::Agent),
        caller("org.quire.Inference", CallerRole::PorterDaemon),
    ] {
        let client = settings_as(&rig, who).await;
        assert!(matches!(
            client.describe().await,
            Err(LiveError::NotPermitted(_))
        ));
        assert!(matches!(
            client.get(&key("accounts.fake-storage.state")).await,
            Err(LiveError::NotPermitted(_))
        ));
        assert!(matches!(
            client.invoke(&key("accounts.fake-storage.remove")).await,
            Err(LiveError::NotPermitted(_))
        ));
    }
    // The stranger is refused too; nothing was removed.
    let stranger = rig.stranger().await;
    let client = ds_settings::live::LiveClient::new(
        &stranger,
        "org.quire.Accounts1",
        &accountd::settings_path(),
    )
    .await
    .expect("client");
    assert!(
        client
            .invoke(&key("accounts.fake-storage.remove"))
            .await
            .is_err()
    );
    assert_eq!(rig.service.registry().accounts.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn describe_lists_the_rows_of_every_account_and_checks_clean() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let (_, grant) = grant_photos(&rig).await;
    let schema = settings(&rig).await.describe().await.expect("schema");
    let paths: Vec<&str> = schema.key.iter().map(|k| k.path.0.as_str()).collect();
    for want in [
        "accounts.fake-storage.service.storage".to_owned(),
        "accounts.fake-storage.remove".to_owned(),
        "accounts.fake-mail.reauth".to_owned(),
        "accounts.clients.microsoft".to_owned(),
        format!("accounts.fake-storage.grant.{grant}"),
    ] {
        assert!(paths.contains(&want.as_str()), "{want} in {paths:?}");
    }
}

async fn found(manager: &ManagerProxy<'_>) -> usize {
    manager
        .query(&storage_need(), "photos", "interactive")
        .await
        .expect("query")
        .len()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_toggle_changes_what_apps_can_find_and_back() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let settings = settings(&rig).await;
    let storage = key("accounts.fake-storage.service.storage");
    assert_eq!(
        settings.get(&storage).await.expect("get"),
        toml::Value::String("on".into())
    );
    let app_conn = rig.client("org.quire.Photos").await;
    let manager = ManagerProxy::new(&app_conn).await.expect("proxy");
    let (code, _) = choose(&app_conn).await;
    assert_eq!(code, 0);
    assert_eq!(found(&manager).await, 1);

    settings
        .set(&storage, &toml::Value::String("off".into()))
        .await
        .expect("off");
    assert_eq!(found(&manager).await, 0);
    assert_eq!(
        settings.get(&storage).await.expect("get"),
        toml::Value::String("off".into())
    );
    assert!(
        rig.service
            .registry()
            .toggles
            .iter()
            .any(|t| t.kind == CapabilityKind::Storage)
    );

    settings
        .set(&storage, &toml::Value::String("on".into()))
        .await
        .expect("on");
    assert_eq!(found(&manager).await, 1);
    assert!(rig.service.registry().toggles.is_empty());
    assert!(matches!(
        settings.set(&storage, &toml::Value::Integer(3)).await,
        Err(LiveError::BadValue(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_a_grant_from_settings_removes_it() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let (_, grant) = grant_photos(&rig).await;
    let row = key(&format!("accounts.fake-storage.grant.{grant}"));
    settings(&rig).await.invoke(&row).await.expect("revoked");
    assert!(rig.service.registry().grants.is_empty());
    assert!(
        rig.audit
            .entries()
            .iter()
            .any(|e| matches!(e.event, AuditEvent::Revoked { .. }))
    );
    // The row is gone; revoking again is an unknown key.
    assert!(matches!(
        settings(&rig).await.invoke(&row).await,
        Err(LiveError::UnknownKey(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn read_outs_and_unknown_keys_are_not_settable() {
    let rig = Rig::start().await;
    let settings = settings(&rig).await;
    assert_eq!(
        settings
            .get(&key("accounts.fake-mail.label"))
            .await
            .expect("label"),
        toml::Value::String("ada@mail.invalid".into())
    );
    assert!(matches!(
        settings
            .set(
                &key("accounts.fake-mail.state"),
                &toml::Value::String("ok".into())
            )
            .await,
        Err(LiveError::NotPermitted(_))
    ));
    assert!(matches!(
        settings.get(&key("accounts.nobody.state")).await,
        Err(LiveError::UnknownKey(_))
    ));
    assert!(matches!(
        settings.get(&key("dock.size")).await,
        Err(LiveError::UnknownKey(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_an_account_wipes_secrets_grants_toggles_and_audits() {
    // Acceptance 8: removal leaves nothing.
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let _ = grant_photos(&rig).await;
    let settings = settings(&rig).await;
    settings
        .set(
            &key("accounts.fake-storage.service.calendar"),
            &toml::Value::String("off".into()),
        )
        .await
        .expect("off");
    let password = SecretKey {
        account: storage_account().id,
        purpose: SecretPurpose::Password,
    };
    assert!(rig.secrets.get(&password).await.is_ok());

    settings
        .invoke(&key("accounts.fake-storage.remove"))
        .await
        .expect("removed");

    let registry = rig.service.registry();
    assert!(
        registry
            .accounts
            .iter()
            .all(|a| a.id != storage_account().id)
    );
    assert!(registry.grants.is_empty());
    assert!(registry.toggles.is_empty());
    assert_eq!(rig.secrets.get(&password).await, Err(SecretsError::Missing));
    let stored = rig.store.stored().expect("saved");
    assert!(stored.accounts.iter().all(|a| a.id != storage_account().id));
    assert!(stored.grants.is_empty() && stored.toggles.is_empty());
    let removed: Vec<_> = rig
        .audit
        .entries()
        .into_iter()
        .filter(|e| e.event == AuditEvent::Removed)
        .collect();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].account, Some(storage_account().id));
    // Its own settings rows are gone, and the other accounts stay.
    let schema = settings_after(&rig).await;
    assert!(
        !schema.iter().any(|p| p.contains("fake-storage")),
        "{schema:?}"
    );
    assert!(schema.iter().any(|p| p.contains("fake-mail")));
}

async fn settings_after(rig: &Rig) -> Vec<String> {
    settings(rig)
        .await
        .describe()
        .await
        .expect("schema")
        .key
        .into_iter()
        .map(|k| k.path.0)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_account_is_told_to_sign_in_again_in_its_state_row() {
    let rig = Rig::start().await;
    assert!(
        rig.service
            .set_state(&mail_account().id, AccountState::NeedsReauth)
            .await
    );
    assert_eq!(
        settings(&rig)
            .await
            .get(&key("accounts.fake-mail.state"))
            .await
            .expect("state"),
        toml::Value::String("needs_reauth".into())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_id_is_written_to_the_users_file_only_by_settings() {
    let dir = std::env::temp_dir().join(format!("accountd-clients-{}", std::process::id()));
    let file = dir.join("porter/clients.toml");
    let options = accountd::Options {
        clients: Some(file.clone()),
        ..Default::default()
    };
    let rig = Rig::start_with(options, SheetHost::quiet()).await;
    let row = KeyPath("accounts.clients.microsoft".into());
    let app = settings_as(&rig, caller("org.example.App", CallerRole::App)).await;
    assert!(
        app.set(&row, &toml::Value::String("evil".into()))
            .await
            .is_err()
    );
    assert!(!file.exists());

    let settings = settings(&rig).await;
    assert_eq!(
        settings.get(&row).await.expect("get"),
        toml::Value::String(String::new())
    );
    settings
        .set(&row, &toml::Value::String("my-client".into()))
        .await
        .expect("set");
    assert_eq!(
        settings.get(&row).await.expect("get"),
        toml::Value::String("my-client".into())
    );
    let text = std::fs::read_to_string(&file).expect("file");
    assert!(
        text.contains("my-client") && text.contains("microsoft"),
        "{text}"
    );
    settings
        .set(&row, &toml::Value::String(String::new()))
        .await
        .expect("clear");
    assert_eq!(
        settings.get(&row).await.expect("get"),
        toml::Value::String(String::new())
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_state_row_is_announced_when_an_account_needs_sign_in_and_when_it_recovers() {
    use zbus::export::futures_core::Stream;
    let rig = Rig::start().await;
    let mail = mail_account();
    let client = settings(&rig).await;
    let mut changes = client.changes().await.expect("changes");
    let other = rig.client("org.quire.Mail").await;
    let manager = ManagerProxy::new(&other).await.expect("proxy");

    for (state, slug) in [
        (AccountState::NeedsReauth, "needs_reauth"),
        (AccountState::Ok, "ok"),
    ] {
        assert!(rig.service.set_state(&mail.id, state).await);
        // Any call ends in a publish, which tells whoever is to be told.
        let _ = manager
            .query(&storage_need(), "photos", "interactive")
            .await;
        let change = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut changes).poll_next(cx)),
        )
        .await
        .expect("a Changed in time")
        .expect("open stream")
        .expect("a change");
        assert_eq!(change.key, key("accounts.fake-mail.state"));
        assert_eq!(change.value, toml::Value::String(slug.into()));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_syncd_cannot_mirror_has_no_sync_row_and_its_switch_is_refused() {
    // `fake-storage` is a WebDAV account: syncd mirrors storage over Graph only.
    let rig = Rig::start().await;
    let client = settings(&rig).await;
    let schema = client.describe().await.expect("schema");
    assert!(schema.key.iter().all(|k| !k.path.0.contains(".sync.")));
    let row = key("accounts.fake-storage.sync.files");
    assert_eq!(
        client.get(&row).await.expect("get"),
        toml::Value::String("off".into())
    );
    assert!(
        client
            .set(&row, &toml::Value::String("on".into()))
            .await
            .is_err()
    );
    assert!(matches!(
        client.set(&row, &toml::Value::Integer(1)).await,
        Err(LiveError::BadValue(_))
    ));
    assert!(rig.service.registry().grants.is_empty());
    // The sync rows never belong to anyone but Settings.
    let app = settings_as(&rig, caller("org.example.App", CallerRole::App)).await;
    assert!(matches!(
        app.set(&row, &toml::Value::String("on".into())).await,
        Err(LiveError::NotPermitted(_))
    ));
}
