//! Acceptance 1, 3 and 8 of design/31 §7.1 on a private bus with scratch HOME and XDG, over the
//! real `AccountService` with the fake providers and in-memory secrets.
//!
//! - 1: an app finds nothing until consent is accepted, then one candidate with its endpoints.
//!   (The add half, `AddAccount` through a family's sign-in, is W3c's: `add_account` is its stub,
//!   so the account is in the registry from the start here; `end_to_end.rs` carries the add.)
//! - 3: no refresh token, password or key is in any message on the bus.
//! - 8: removal leaves nothing: secrets, grants, toggles, registry row, signals, audit.

use crate::common;

use common::*;
use ds_settings::schema::KeyPath;
use porter_core::{SecretKey, SecretPurpose};
use porter_dbus::{AccountProxy, GrantsProxy, ManagerProxy, PeerProxy, TokensProxy, account_path};
use porter_secrets::{Secrets, SecretsError};
use zbus::export::futures_core::Stream;
use zbus::fdo::MonitoringProxy;
use zbus::zvariant::ObjectPath;

#[tokio::test(flavor = "multi_thread")]
async fn acceptance_1_nothing_until_consent_then_one_candidate_with_endpoints() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let first = rig.client("org.quire.Photos").await;
    let second = rig.client("org.quire.Mail").await;
    let query = |conn: zbus::Connection| async move {
        ManagerProxy::new(&conn)
            .await
            .expect("proxy")
            .query(&storage_need(), "photos", "interactive")
            .await
            .expect("query")
    };
    assert!(query(first.clone()).await.is_empty());
    assert!(query(second.clone()).await.is_empty());

    let (code, _) = choose(&first).await;
    assert_eq!(code, 0);

    let found = query(first.clone()).await;
    assert_eq!(found.len(), 1);
    let candidate = porter_dbus::candidate_from_dbus(found[0].clone()).expect("candidate");
    assert_eq!(candidate.account, storage_account().id);
    assert!(
        candidate.endpoints.iter().any(|e| e
            .url
            .as_str()
            .contains("cloud.invalid/remote.php/dav/files")),
        "{:?}",
        candidate.endpoints
    );
    // The consent was the first app's alone.
    assert!(query(second).await.is_empty());
}

/// Every message on the bus, from the moment the monitor is set.
struct Tap(zbus::MessageStream);

impl Tap {
    async fn start(rig: &Rig) -> Self {
        let monitor = rig.bus.connect().await;
        MonitoringProxy::new(&monitor)
            .await
            .expect("proxy")
            .become_monitor(&[], 0)
            .await
            .expect("monitor");
        Self(zbus::MessageStream::from(monitor))
    }

    /// What has gone by so far: each message's bytes.
    async fn drain(&mut self) -> Vec<Vec<u8>> {
        let mut seen = Vec::new();
        loop {
            let next = tokio::time::timeout(
                std::time::Duration::from_millis(300),
                std::future::poll_fn(|cx| std::pin::Pin::new(&mut self.0).poll_next(cx)),
            )
            .await;
            match next {
                Ok(Some(Ok(message))) => seen.push(message.data().to_vec()),
                _ => return seen,
            }
        }
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[tokio::test(flavor = "multi_thread")]
async fn acceptance_3_no_secret_is_in_any_message_on_the_bus() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let mut tap = Tap::start(&rig).await;

    // Everything an app and the other daemons can ask, for the accounts that hold secrets.
    let (photos, grant) = grant_photos(&rig).await;
    let manager = ManagerProxy::new(&photos).await.expect("proxy");
    manager
        .query(&storage_need(), "photos", "interactive")
        .await
        .expect("query");
    manager
        .availability(&storage_need(), "photos", "interactive")
        .await
        .expect("availability");
    GrantsProxy::new(&photos)
        .await
        .expect("proxy")
        .list()
        .await
        .expect("list");
    let token = TokensProxy::new(&photos)
        .await
        .expect("proxy")
        .issue_token(grant.as_str(), "webdav")
        .await
        .expect("token");
    assert!(
        token.1.starts_with("fake:"),
        "a token is on the bus: the scan can see values"
    );
    let account = AccountProxy::builder(&photos)
        .path(ObjectPath::try_from(account_path(&storage_account().id)).expect("path"))
        .expect("path")
        .build()
        .await
        .expect("proxy");
    let _ = (
        account.id().await,
        account.capabilities().await,
        account.state().await,
    );
    let inferd = rig
        .client_as(caller(
            "org.quire.Inference",
            porter_dbus::CallerRole::PorterDaemon,
        ))
        .await;
    PeerProxy::new(&inferd)
        .await
        .expect("proxy")
        .verdicts(
            &("org.quire.Photos".to_owned(), "flatpak".to_owned()),
            &storage_need(),
            "photos",
            "interactive",
        )
        .await
        .expect("verdicts");
    // Settings reads every key of every account, the mail account (OAuth) included.
    let settings = settings(&rig).await;
    for row in settings.describe().await.expect("schema").key {
        let _ = settings.get(&row.path).await;
    }
    settings
        .set(
            &KeyPath("accounts.fake-mail.service.mail".into()),
            &toml::Value::String("off".into()),
        )
        .await
        .expect("toggle");

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
        seen.iter().any(|m| contains(m, "fake:fake-storage:webdav")),
        "positive control: the scan finds a value that did cross"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn acceptance_8_removal_leaves_nothing_and_the_holder_is_told() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let (photos, grant) = grant_photos(&rig).await;
    let mut hears = listen(&photos).await;
    let mail = SecretKey {
        account: mail_account().id,
        purpose: SecretPurpose::OAuthRefresh,
    };
    let password = SecretKey {
        account: storage_account().id,
        purpose: SecretPurpose::Password,
    };
    let settings = settings(&rig).await;
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
    assert!(registry.grants.is_empty());
    let stored = rig.store.stored().expect("stored");
    assert!(stored.accounts.iter().all(|a| a.id != storage_account().id));
    // The grant holder cannot see it any more: no candidate, no object, no grant.
    let manager = ManagerProxy::new(&photos).await.expect("proxy");
    assert!(
        manager
            .query(&storage_need(), "photos", "interactive")
            .await
            .expect("query")
            .is_empty()
    );
    let account = AccountProxy::builder(&photos)
        .path(ObjectPath::try_from(account_path(&storage_account().id)).expect("path"))
        .expect("path")
        .build()
        .await
        .expect("proxy");
    assert!(account.id().await.is_err());
    let issued = TokensProxy::new(&photos)
        .await
        .expect("proxy")
        .issue_token(grant.as_str(), "webdav")
        .await
        .expect_err("grant gone");
    assert_eq!(
        error_name(&issued),
        refusal_name(porter_core::wire::Refusal::UnknownGrant)
    );
    // The holder was told, once each.
    let told = heard(&mut hears, std::time::Duration::from_millis(500)).await;
    assert_eq!(
        told.iter().filter(|n| *n == "AccountRemoved").count(),
        1,
        "{told:?}"
    );
    // The audit names the removal.
    assert!(
        rig.audit
            .entries()
            .iter()
            .any(|e| e.event == porter_core::audit::AuditEvent::Removed
                && e.account == Some(storage_account().id))
    );
}
