//! `Manager.Adopt`: the `[adopt]` table decides who may ask, the daemon reads the old items
//! itself from a legacy store (a fixture of mailo's entries, never the real keyring) and files
//! them as porter secrets.

mod common;

use accountd::{AdoptConfig, AdoptTable, MemoryLegacy, Options};
use common::*;
use porter_core::audit::AuditEvent;
use porter_core::wire::{LegacyItem, LegacyRef, Refusal};
use porter_core::{
    AccountId, AccountLabel, Credential, ProviderId, SecretKey, SecretPurpose, SecretText,
};
use porter_dbus::{CallerRole, Details, ManagerProxy, legacy_to_dbus};
use porter_secrets::Secrets;
use std::sync::Arc;

const UUID: &str = "67e55044-10b1-426f-9247-bb680e5fe0c8";

fn mailo_entries() -> MemoryLegacy {
    let entry = |what: &str| format!("{UUID}:{what}");
    MemoryLegacy::default()
        .with(
            "mailo",
            &entry("incoming"),
            &format!(r#"{{"kind":"password","v":"{APP_PASSWORD}"}}"#),
        )
        .with("mailo", &entry("outgoing"), r#"{"kind":"password","v":"S3CRET-SMTP"}"#)
        .with(
            "mailo",
            &entry("oauth"),
            &format!(
                r#"{{"kind":"oauth","v":{{"access":"{ACCESS_TOKEN}","refresh":"{REFRESH_TOKEN}","expires_at":"2026-10-05T12:30:00Z"}}}}"#
            ),
        )
        // Another service's entry for the same id: never read for this caller.
        .with("other", &entry("incoming"), r#"{"kind":"password","v":"NOT-MINE"}"#)
}

fn options(store: Option<MemoryLegacy>) -> Options {
    Options {
        adopt: AdoptConfig {
            table: AdoptTable::from_config("[adopt]\n\"org.quire.Mail\" = \"mailo\"\n")
                .expect("table"),
            store: store.map(|s| Arc::new(s) as Arc<_>),
        },
        ..Default::default()
    }
}

fn legacy(items: Vec<LegacyItem>) -> Details {
    legacy_to_dbus(&LegacyRef {
        account: AccountId::parse(UUID).expect("id"),
        provider: ProviderId::parse("fake-mail").expect("id"),
        label: AccountLabel("ada@mail.invalid".into()),
        endpoints: vec![],
        items,
    })
}

fn key(purpose: SecretPurpose) -> SecretKey {
    SecretKey {
        account: AccountId::parse(UUID).expect("id"),
        purpose,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_named_app_adopts_its_legacy_account_and_the_secrets_are_filed() {
    let rig = Rig::start_with(options(Some(mailo_entries())), SheetHost::quiet()).await;
    let mail = rig.client("org.quire.Mail").await;
    let manager = ManagerProxy::new(&mail).await.expect("proxy");
    let id = manager
        .adopt(&legacy(vec![
            LegacyItem::Incoming,
            LegacyItem::Outgoing,
            LegacyItem::OAuth,
        ]))
        .await
        .expect("adopted");
    assert_eq!(id, UUID);

    let registry = rig.service.registry();
    let account = registry
        .accounts
        .iter()
        .find(|a| a.id.as_str() == UUID)
        .expect("the account");
    assert_eq!(account.label.0, "ada@mail.invalid");
    assert_eq!(account.provider.as_str(), "fake-mail");
    assert!(!account.capabilities.is_empty());
    assert!(
        rig.store
            .stored()
            .expect("saved")
            .accounts
            .iter()
            .any(|a| a.id.as_str() == UUID)
    );

    assert_eq!(
        rig.secrets.get(&key(SecretPurpose::IncomingPassword)).await,
        Ok(Credential::Password(SecretText::new(APP_PASSWORD)))
    );
    assert_eq!(
        rig.secrets.get(&key(SecretPurpose::OutgoingPassword)).await,
        Ok(Credential::Password(SecretText::new("S3CRET-SMTP")))
    );
    assert!(matches!(
        rig.secrets.get(&key(SecretPurpose::OAuthRefresh)).await,
        Ok(Credential::OAuth {
            expires_at: porter_core::UnixSeconds(1_791_203_400),
            ..
        })
    ));
    let audited: Vec<_> = rig
        .audit
        .entries()
        .into_iter()
        .filter(|e| e.event == AuditEvent::Adopted)
        .collect();
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].app, Some(app("org.quire.Mail")));

    // Asking again is the same account, not a second one.
    let again = manager
        .adopt(&legacy(vec![LegacyItem::Incoming]))
        .await
        .expect("again");
    assert_eq!(again, UUID);
    assert_eq!(
        rig.service
            .registry()
            .accounts
            .iter()
            .filter(|a| a.id.as_str() == UUID)
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_the_table_does_not_name_is_denied_and_nothing_is_read() {
    let rig = Rig::start_with(options(Some(mailo_entries())), SheetHost::quiet()).await;
    let other = rig.client("org.example.Other").await;
    let err = ManagerProxy::new(&other)
        .await
        .expect("proxy")
        .adopt(&legacy(vec![LegacyItem::Incoming]))
        .await
        .expect_err("denied");
    assert_eq!(error_name(&err), refusal_name(Refusal::Denied));
    assert!(
        rig.service
            .registry()
            .accounts
            .iter()
            .all(|a| a.id.as_str() != UUID)
    );
    assert_eq!(
        rig.secrets.get(&key(SecretPurpose::Password)).await,
        Err(porter_secrets::SecretsError::Missing)
    );

    // An agent that is somehow named in the table is refused as well.
    let agent = rig
        .client_as(caller("org.quire.Mail", CallerRole::Agent))
        .await;
    let err = ManagerProxy::new(&agent)
        .await
        .expect("proxy")
        .adopt(&legacy(vec![LegacyItem::Incoming]))
        .await
        .expect_err("denied");
    assert_eq!(error_name(&err), refusal_name(Refusal::Denied));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listed_item_that_is_not_there_adopts_nothing() {
    let rig = Rig::start_with(options(Some(mailo_entries())), SheetHost::quiet()).await;
    let mail = rig.client("org.quire.Mail").await;
    let err = ManagerProxy::new(&mail)
        .await
        .expect("proxy")
        .adopt(&legacy(vec![LegacyItem::Incoming, LegacyItem::AddressBook]))
        .await
        .expect_err("missing carddav");
    assert_eq!(error_name(&err), refusal_name(Refusal::Unavailable));
    assert!(
        rig.service
            .registry()
            .accounts
            .iter()
            .all(|a| a.id.as_str() != UUID)
    );
    assert_eq!(
        rig.secrets.get(&key(SecretPurpose::IncomingPassword)).await,
        Err(porter_secrets::SecretsError::Missing)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_legacy_store_adopt_is_unavailable() {
    let rig = Rig::start_with(options(None), SheetHost::quiet()).await;
    let mail = rig.client("org.quire.Mail").await;
    let err = ManagerProxy::new(&mail)
        .await
        .expect("proxy")
        .adopt(&legacy(vec![LegacyItem::Incoming]))
        .await
        .expect_err("unavailable");
    assert_eq!(error_name(&err), refusal_name(Refusal::Unavailable));
}

#[tokio::test(flavor = "multi_thread")]
async fn items_the_app_already_filed_itself_are_skipped_and_kept() {
    // mailo's own adoption (E2) filed the incoming password and the OAuth token and deleted their
    // legacy entries; only the outgoing one is still in the old store.
    let entry = |what: &str| format!("{UUID}:{what}");
    let store = MemoryLegacy::default().with(
        "mailo",
        &entry("outgoing"),
        r#"{"kind":"password","v":"S3CRET-SMTP"}"#,
    );
    let rig = Rig::start_with(options(Some(store)), SheetHost::quiet()).await;
    let filed_by_mailo = Credential::Password(SecretText::new("FILED-BY-MAILO"));
    let rotated = Credential::Password(SecretText::new("ROTATED-SINCE"));
    rig.secrets
        .put(&key(SecretPurpose::IncomingPassword), &filed_by_mailo)
        .await
        .expect("filed");
    rig.secrets
        .put(&key(SecretPurpose::OAuthRefresh), &rotated)
        .await
        .expect("filed");

    let mail = rig.client("org.quire.Mail").await;
    let id = ManagerProxy::new(&mail)
        .await
        .expect("proxy")
        .adopt(&legacy(vec![
            LegacyItem::Incoming,
            LegacyItem::Outgoing,
            LegacyItem::OAuth,
        ]))
        .await
        .expect("adopted");
    assert_eq!(id, UUID);

    // What mailo filed is not replaced; what was still legacy is filed now.
    assert_eq!(
        rig.secrets.get(&key(SecretPurpose::IncomingPassword)).await,
        Ok(filed_by_mailo)
    );
    assert_eq!(
        rig.secrets.get(&key(SecretPurpose::OAuthRefresh)).await,
        Ok(rotated)
    );
    assert_eq!(
        rig.secrets.get(&key(SecretPurpose::OutgoingPassword)).await,
        Ok(Credential::Password(SecretText::new("S3CRET-SMTP")))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_item_neither_in_the_old_store_nor_filed_still_refuses() {
    let rig = Rig::start_with(options(Some(MemoryLegacy::default())), SheetHost::quiet()).await;
    rig.secrets
        .put(
            &key(SecretPurpose::OAuthRefresh),
            &Credential::Password(SecretText::new("FILED")),
        )
        .await
        .expect("filed");
    let mail = rig.client("org.quire.Mail").await;
    let err = ManagerProxy::new(&mail)
        .await
        .expect("proxy")
        .adopt(&legacy(vec![LegacyItem::OAuth, LegacyItem::Incoming]))
        .await
        .expect_err("incoming is nowhere");
    assert_eq!(error_name(&err), refusal_name(Refusal::Unavailable));
    // The refusal leaves what the app filed alone.
    assert!(
        rig.secrets
            .get(&key(SecretPurpose::OAuthRefresh))
            .await
            .is_ok()
    );
}
