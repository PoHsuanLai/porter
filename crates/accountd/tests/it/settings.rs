//! `org.quire.SettingsModule1` on accountd: only the Settings role, the rows of PLAN §2.5 (the
//! identities and Tailscale, the apps using each identity, the client ids), and what each `Set`
//! does to the registry. The service switches, grant rows and sync switches are gone with the
//! grants: inferd's and syncd's modules list those.

use crate::common;

use common::*;
use ds_settings::live::{AnyOwner, LiveClient, LiveError, Snapshot};
use ds_settings::schema::KeyPath;
use porter_core::{AccountState, SecretKey, SecretPurpose};
use porter_dbus::accounts::IdentitiesProxy;
use porter_dbus::callers::CallerRole;
use porter_secrets::{Secrets, SecretsError};
use porter_service::audit::AuditEvent;
use test_kit::Scratch;

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
            client.snapshot().await,
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
        AnyOwner,
    )
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
async fn describe_lists_the_identities_and_their_actions_and_nothing_of_the_old_model() {
    let rig = Rig::start().await;
    let schema = settings(&rig).await.describe().await.expect("schema");
    assert!(schema.check().is_ok());
    let paths: Vec<&str> = schema.key.iter().map(|k| k.path.0.as_str()).collect();
    for want in [
        "accounts.fake-storage.remove",
        "accounts.fake-mail.reauth",
        "accounts.fake-mail.state",
        "accounts.clients.microsoft",
    ] {
        assert!(paths.contains(&want), "{want} in {paths:?}");
    }
    // No service switch, grant or sync row, and the AI account is inferd's to list.
    for gone in [".service.", ".grant.", ".sync.", ".sign_out", "fake-llm"] {
        assert!(
            !paths.iter().any(|p| p.contains(gone)),
            "{gone} in {paths:?}"
        );
    }
}

/// The apps that signed in with an identity are listed under it by name, read-only.
#[tokio::test(flavor = "multi_thread")]
async fn the_apps_using_an_identity_are_listed_under_it_as_read_only_rows() {
    let rig = Rig::start().await;
    let photos = rig.client("org.quire.Photos").await;
    IdentitiesProxy::new(&photos)
        .await
        .expect("proxy")
        .register_use("fake-storage")
        .await
        .expect("register");
    let settings = settings(&rig).await;
    let row = key("accounts.fake-storage.used_by.0");
    assert_eq!(
        settings.get(&row).await.expect("row"),
        toml::Value::String("org.quire.Photos".into())
    );
    assert!(
        settings
            .describe()
            .await
            .expect("schema")
            .key
            .iter()
            .any(|k| k.path == row)
    );
    assert!(matches!(
        settings.set(&row, &toml::Value::String("x".into())).await,
        Err(LiveError::NotPermitted(_))
    ));
    assert!(matches!(
        settings.get(&key("accounts.fake-storage.used_by.1")).await,
        Err(LiveError::UnknownKey(_))
    ));
}

/// What a snapshot promises: every row has its value and every value has its row.
fn agrees(snapshot: &Snapshot) -> bool {
    snapshot
        .schema
        .key
        .iter()
        .all(|spec| snapshot.values.contains_key(&spec.path))
        && snapshot
            .values
            .keys()
            .all(|path| snapshot.schema.key.iter().any(|spec| spec.path == *path))
}

/// `Snapshot` answers the schema and the values from one read of the registry, so its rows and
/// its values agree even when an account is removed while it is read, which `Describe` then a
/// `Get` per key (`snapshot_by_parts`) can get wrong. Whatever the interleaving, every snapshot
/// taken beside the removal must agree; before or after it, it lists the account whole or not at
/// all.
#[tokio::test(flavor = "multi_thread")]
async fn a_snapshot_agrees_with_itself_while_an_account_is_removed_beside_it() {
    let rig = Rig::start().await;
    let reader = settings(&rig).await;
    let remover = settings(&rig).await;
    let before = reader.snapshot().await.expect("snapshot");
    assert!(agrees(&before));
    assert!(
        before
            .schema
            .key
            .iter()
            .any(|k| k.path.0 == "accounts.fake-storage.state")
    );

    let removal = async {
        remover
            .invoke(&key("accounts.fake-storage.remove"))
            .await
            .expect("removed");
    };
    let reads = async {
        let mut taken = Vec::new();
        for _ in 0..20 {
            taken.push(reader.snapshot().await.expect("snapshot"));
        }
        taken
    };
    let ((), taken) = tokio::join!(removal, reads);
    for snapshot in &taken {
        assert!(agrees(snapshot));
        // The account's rows are all there or all gone.
        let rows = snapshot
            .schema
            .key
            .iter()
            .filter(|k| k.path.0.starts_with("accounts.fake-storage."))
            .count();
        assert!(rows == 0 || rows == before_rows(&before), "{rows}");
    }
    let after = reader.snapshot().await.expect("snapshot");
    assert!(agrees(&after));
    assert!(
        !after
            .schema
            .key
            .iter()
            .any(|k| k.path.0.starts_with("accounts.fake-storage."))
    );
}

fn before_rows(snapshot: &Snapshot) -> usize {
    snapshot
        .schema
        .key
        .iter()
        .filter(|k| k.path.0.starts_with("accounts.fake-storage."))
        .count()
}

/// Quiet, the one-read snapshot and the default one read by parts are the same thing.
#[tokio::test(flavor = "multi_thread")]
async fn a_snapshot_is_what_describe_and_get_give_when_nothing_changes() {
    let rig = Rig::start().await;
    let client: LiveClient = settings(&rig).await;
    let once = client.snapshot().await.expect("snapshot");
    let parts = client.snapshot_by_parts().await.expect("by parts");
    assert_eq!(once, parts);
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
    for row in ["provider", "group", "sign_in"] {
        let path = format!("accounts.fake-mail.{row}");
        assert!(matches!(
            settings.get(&key(&path)).await,
            Ok(toml::Value::String(_))
        ));
        assert!(
            matches!(
                settings
                    .set(&key(&path), &toml::Value::String("x".into()))
                    .await,
                Err(LiveError::NotPermitted(_))
            ),
            "{path}"
        );
    }
    // The rows of the old model are no keys at all.
    for gone in [
        "accounts.fake-storage.service.storage",
        "accounts.fake-storage.grant.g1",
        "accounts.fake-storage.sync.files",
        "accounts.fake-storage.sign_out",
    ] {
        assert!(
            matches!(
                settings.get(&key(gone)).await,
                Err(LiveError::UnknownKey(_))
            ),
            "{gone}"
        );
    }
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
async fn removing_an_account_wipes_secrets_and_audits() {
    // Acceptance 8: removal leaves nothing.
    let rig = Rig::start().await;
    let settings = settings(&rig).await;
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
    assert_eq!(rig.secrets.get(&password).await, Err(SecretsError::Missing));
    let stored = rig.store.stored().expect("saved");
    assert!(stored.accounts.iter().all(|a| a.id != storage_account().id));
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
    let dir = Scratch::new("accountd-clients");
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
    // Both changes are audited, never with the id (sec-5); the refused one is not.
    use porter_service::audit::ClientIdChange;
    let changed: Vec<_> = rig
        .audit
        .entries()
        .into_iter()
        .filter_map(|e| match e.event {
            AuditEvent::ClientIdChanged { issuer, change } => Some((issuer, change)),
            _ => None,
        })
        .collect();
    let microsoft = "microsoft".to_owned();
    assert_eq!(
        changed,
        [
            (microsoft.clone(), ClientIdChange::Set),
            (microsoft, ClientIdChange::Cleared)
        ]
    );
    assert!(
        rig.audit
            .entries()
            .iter()
            .all(|e| !format!("{e:?}").contains("my-client")),
        "no entry holds the id"
    );
}

/// ux-3: Settings sets a Google sign-in key and its secret; the families, which read the same
/// file at every sign-in (ux-2), then have a Google client with both.
#[tokio::test(flavor = "multi_thread")]
async fn a_google_key_and_its_secret_set_in_settings_are_the_families_google_client() {
    let dir = Scratch::new("accountd-google-client");
    let file = dir.join("porter/clients.toml");
    let options = accountd::Options {
        clients: Some(file.clone()),
        ..Default::default()
    };
    let rig = Rig::start_with(options, SheetHost::quiet()).await;
    let settings = settings(&rig).await;
    let (id_row, secret_row) = (
        KeyPath("accounts.clients.google".into()),
        KeyPath("accounts.clients.google.secret".into()),
    );
    let schema = settings.describe().await.expect("schema");
    for row in [&id_row, &secret_row] {
        assert!(schema.key.iter().any(|k| k.path == *row), "{}", row.0);
    }
    // A secret alone has no row to go in.
    assert!(matches!(
        settings
            .set(&secret_row, &toml::Value::String("GOCSPX-early".into()))
            .await,
        Err(LiveError::BadValue(_))
    ));
    settings
        .set(
            &id_row,
            &toml::Value::String("mine.apps.googleusercontent.com".into()),
        )
        .await
        .expect("key");
    settings
        .set(&secret_row, &toml::Value::String("GOCSPX-mine".into()))
        .await
        .expect("secret");
    assert_eq!(
        settings.get(&secret_row).await.expect("get"),
        toml::Value::String("GOCSPX-mine".into())
    );
    let families = porter_family_oauth::ClientFiles {
        shipped: dir.join("none.toml"),
        own: file.clone(),
    }
    .read();
    // The channel is the build's (development in a debug build).
    let client = [
        porter_provider::ClientChannel::Development,
        porter_provider::ClientChannel::Stable,
    ]
    .into_iter()
    .find_map(|channel| families.lookup(porter_provider::Issuer::Google, channel))
    .expect("a google client");
    assert_eq!(client.client_id.0, "mine.apps.googleusercontent.com");
    assert_eq!(
        client.client_secret.as_ref().map(|s| s.expose().to_owned()),
        Some("GOCSPX-mine".to_owned())
    );
}

/// The next word said on `row` (the action's own `Set` answers with a button's `false`).
async fn word_on(changes: &mut ds_settings::live::Changes, row: &KeyPath) -> String {
    use zbus::export::futures_core::Stream;
    loop {
        let change = tokio::time::timeout(
            test_kit::GENEROUS,
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut *changes).poll_next(cx)),
        )
        .await
        .expect("a Changed in time")
        .expect("open stream")
        .expect("a change");
        if change.key == *row && change.value.is_str() {
            return change.value.as_str().unwrap_or_default().to_owned();
        }
    }
}

/// ux-8: a "Sign in again" pressed in Settings that does not end signed in is said on its row,
/// and a program on this computer has no such row and refuses the action.
#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_again_that_ends_without_signing_in_is_said_on_its_row() {
    // The mail account's provider is not loaded: its sign-in fails, and the person closes the
    // sheet that says so.
    let rig = Rig::start_over(
        Default::default(),
        SheetHost::answering(porter_sheet::SheetInput::Dismiss),
        vec![porter_fake::cloud_provider(), porter_fake::llm_provider()],
    )
    .await;
    let client = settings(&rig).await;
    let mut changes = client.changes().await.expect("changes");
    let row = key("accounts.fake-mail.reauth");
    client.invoke(&row).await.expect("the action is under way");
    assert_eq!(word_on(&mut changes, &row).await, "failed");

    let local = key("accounts.fake-llm.reauth");
    let schema = client.describe().await.expect("schema");
    assert!(!schema.key.iter().any(|k| k.path == local));
    assert!(client.invoke(&local).await.is_err());
}
