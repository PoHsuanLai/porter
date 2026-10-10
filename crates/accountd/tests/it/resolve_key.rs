//! `Peer.ResolveKey` on a private bus: a porter daemon gets the API key of a granted Llm account
//! on a sealed memfd, nobody else gets anything, and no message on the bus carries the key
//! (acceptance 7 of design/31 §7.1, with a positive control).

use crate::common;

use common::*;
use porter_core::audit::AuditEvent;
use porter_core::capability::LlmFeature;
use porter_core::consent::{ConsentAnswer, GrantScope};
use porter_core::need::LlmNeed;
use porter_core::sheet::SheetInput;
use porter_core::{
    AccountState, Audience, Credential, GrantId, Need, SecretKey, SecretPurpose, SecretText, Tokens,
};
use porter_dbus::{CallerRole, GrantsProxy, ManagerProxy, PeerProxy, need_to_dbus};
use porter_secrets::Secrets;
use rustix::fs::{SealFlags, fcntl_add_seals, fcntl_get_seals, ftruncate};
use std::collections::BTreeSet;
use std::io::{Read, Seek, Write};

const KEY: &str = "sk-or-v1-S3CRET-CLOUD-KEY-0123456789";

fn llm_need() -> porter_dbus::NeedArg {
    need_to_dbus(&Need::Llm(LlmNeed::new(
        BTreeSet::from([LlmFeature::Chat]),
        Tokens(0),
    )))
}

fn allowing_llm() -> SheetHost {
    SheetHost::answering(SheetInput::Answer(ConsentAnswer::Allow {
        account: porter_fake::llm_account().id,
        scope: GrantScope::Always,
    }))
}

fn llm_key() -> SecretKey {
    SecretKey {
        account: porter_fake::llm_account().id,
        purpose: SecretPurpose::ApiKey,
    }
}

/// A rig whose Llm account holds `KEY`, with the Companion's grant for it.
async fn rig_with_grant() -> (Rig, GrantId) {
    let rig = Rig::start_with(Default::default(), allowing_llm()).await;
    rig.secrets
        .put(&llm_key(), &Credential::ApiKey(SecretText::new(KEY)))
        .await
        .expect("key filed");
    let companion = rig.client("org.quire.Companion").await;
    let mut sheet = porter_dbus::Sheet::subscribe(&companion)
        .await
        .expect("subscribe");
    let path = ManagerProxy::new(&companion)
        .await
        .expect("proxy")
        .choose(&llm_need(), "prompt", "interactive", "", &sheet.options())
        .await
        .expect("choose");
    let (code, results) = sheet.response(&path).await.expect("response");
    assert_eq!(code, 0, "{results:?}");
    (rig, grant_in(&results))
}

async fn inferd(rig: &Rig) -> zbus::Connection {
    rig.client_as(caller("org.quire.Inference", CallerRole::PorterDaemon))
        .await
}

async fn peer_of(connection: &zbus::Connection) -> PeerProxy<'_> {
    PeerProxy::new(connection).await.expect("proxy")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_porter_daemon_gets_the_key_on_a_sealed_memfd_and_the_release_is_audited() {
    let (rig, grant) = rig_with_grant().await;
    let daemon = inferd(&rig).await;
    let fd = peer_of(&daemon)
        .await
        .resolve_key(grant.as_str())
        .await
        .expect("resolve");
    let fd = std::os::fd::OwnedFd::from(fd);
    assert_eq!(
        fcntl_get_seals(&fd).expect("seals"),
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL
    );
    let mut file = std::fs::File::from(fd);
    let mut text = String::new();
    file.read_to_string(&mut text)
        .expect("readable from the start");
    assert_eq!(text, KEY);
    // Sealed: it cannot be written, grown, shrunk or unsealed, by the reader or any holder.
    assert!(file.write_all(b"x").is_err());
    assert!(ftruncate(&file, 0).is_err());
    assert!(ftruncate(&file, 1 << 16).is_err());
    assert!(fcntl_add_seals(&file, SealFlags::empty()).is_err());
    file.rewind().expect("rewind");
    let mut again = String::new();
    file.read_to_string(&mut again).expect("read");
    assert_eq!(again, KEY);

    // One audit line for the release: the app the grant is for, which account, which grant;
    // never the key. A second release is a second line.
    let released = || {
        rig.audit
            .entries()
            .into_iter()
            .filter(|e| matches!(&e.event, AuditEvent::KeyResolved { .. }))
            .collect::<Vec<_>>()
    };
    assert_eq!(released().len(), 1, "{:?}", rig.audit.entries());
    let entry = released().remove(0);
    assert_eq!(
        entry.app.as_ref().map(|a| a.name.as_str()),
        Some("org.quire.Companion")
    );
    assert_eq!(entry.account, Some(porter_fake::llm_account().id));
    assert_eq!(
        entry.event,
        AuditEvent::KeyResolved {
            grant: grant.clone(),
            audience: Audience("org.quire.Companion".into()),
        }
    );
    peer_of(&daemon)
        .await
        .resolve_key(grant.as_str())
        .await
        .expect("resolve again");
    assert_eq!(released().len(), 2);
    // No other event stands in for it any more.
    assert!(
        rig.audit
            .entries()
            .iter()
            .all(|e| !matches!(&e.event, AuditEvent::TokenIssued { .. }))
    );
    let everything = serde_json::to_string(&rig.audit.entries()).expect("json");
    assert!(!everything.contains(KEY), "{everything}");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_porter_daemon_may_ask_even_with_a_valid_grant() {
    let (rig, grant) = rig_with_grant().await;
    for refused in [
        rig.client_as(caller("org.quire.Companion", CallerRole::App))
            .await,
        rig.client_as(caller("org.quire.Companion", CallerRole::Agent))
            .await,
        rig.client_as(caller("org.quire.Settings", CallerRole::Settings))
            .await,
        rig.client_as(caller("org.example.Sheets", CallerRole::SheetHost))
            .await,
        rig.stranger().await,
    ] {
        let err = PeerProxy::new(&refused)
            .await
            .expect("proxy")
            .resolve_key(grant.as_str())
            .await
            .expect_err("refused");
        assert_eq!(error_name(&err), ACCESS_DENIED);
    }
    assert!(
        rig.audit
            .entries()
            .iter()
            .all(|e| !matches!(&e.event, AuditEvent::KeyResolved { .. }))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_that_does_not_open_a_key_is_refused_with_its_reason() {
    use porter_core::wire::Refusal;
    let (rig, grant) = rig_with_grant().await;
    let daemon = inferd(&rig).await;
    let peer = peer_of(&daemon).await;
    let name = |refusal| refusal_name(refusal);

    // A grant nobody holds, and text that is no grant id.
    let err = peer.resolve_key("grant-404").await.expect_err("unknown");
    assert_eq!(error_name(&err), name(Refusal::UnknownGrant));
    let err = peer
        .resolve_key("not a grant")
        .await
        .expect_err("malformed");
    assert!(error_name(&err).contains("InvalidArgs"), "{err:?}");

    // A grant for the storage account is not for a key.
    let storage = Rig::start_with(Default::default(), allowing_host()).await;
    let (_photos, storage_grant) = grant_photos(&storage).await;
    let storage_daemon = inferd(&storage).await;
    let err = peer_of(&storage_daemon)
        .await
        .resolve_key(storage_grant.as_str())
        .await
        .expect_err("not llm");
    assert_eq!(error_name(&err), name(Refusal::AudienceNotGranted));

    // The service turned off for the account.
    let settings = settings(&rig).await;
    settings
        .set(
            &key("accounts.fake-llm.service.llm"),
            &toml::Value::String("off".into()),
        )
        .await
        .expect("toggle");
    let err = peer.resolve_key(grant.as_str()).await.expect_err("off");
    assert_eq!(error_name(&err), name(Refusal::Denied));
    settings
        .set(
            &key("accounts.fake-llm.service.llm"),
            &toml::Value::String("on".into()),
        )
        .await
        .expect("toggle back");
    peer.resolve_key(grant.as_str()).await.expect("on again");

    // An account that must be signed in again, and a key that is no longer filed.
    let llm = porter_fake::llm_account().id;
    assert!(rig.service.set_state(&llm, AccountState::NeedsReauth).await);
    let err = peer.resolve_key(grant.as_str()).await.expect_err("reauth");
    assert_eq!(error_name(&err), name(Refusal::NeedsReauth));
    assert!(rig.service.set_state(&llm, AccountState::Ok).await);
    rig.secrets.delete(&llm_key()).await.expect("delete");
    let err = peer.resolve_key(grant.as_str()).await.expect_err("no key");
    assert_eq!(error_name(&err), name(Refusal::NeedsReauth));

    // A withdrawn grant opens nothing.
    rig.secrets
        .put(&llm_key(), &Credential::ApiKey(SecretText::new(KEY)))
        .await
        .expect("key filed");
    let companion = rig.client("org.quire.Companion").await;
    GrantsProxy::new(&companion)
        .await
        .expect("proxy")
        .revoke(grant.as_str())
        .await
        .expect("revoke");
    let err = peer.resolve_key(grant.as_str()).await.expect_err("revoked");
    assert_eq!(error_name(&err), name(Refusal::UnknownGrant));
}

#[tokio::test(flavor = "multi_thread")]
async fn acceptance_7_no_key_is_in_any_message_on_the_bus() {
    let (rig, grant) = rig_with_grant().await;
    let mut tap = Tap::start(&rig.bus).await;
    let daemon = inferd(&rig).await;
    let peer = peer_of(&daemon).await;

    // Everything the person's apps and the daemons can ask about the account with the key.
    let companion = rig.client("org.quire.Companion").await;
    let manager = ManagerProxy::new(&companion).await.expect("proxy");
    manager
        .query(&llm_need(), "prompt", "interactive")
        .await
        .expect("query");
    manager
        .availability(&llm_need(), "prompt", "interactive")
        .await
        .expect("availability");
    GrantsProxy::new(&companion)
        .await
        .expect("proxy")
        .list()
        .await
        .expect("list");
    peer.verdicts(
        &("org.quire.Companion".to_owned(), "flatpak".to_owned()),
        &llm_need(),
        "prompt",
        "interactive",
    )
    .await
    .expect("verdicts");
    let settings = settings(&rig).await;
    for row in settings.describe().await.expect("schema").key {
        let _ = settings.get(&row.path).await;
    }
    // The key itself, to a porter daemon: on the fd.
    let fd = peer.resolve_key(grant.as_str()).await.expect("resolve");
    let mut text = String::new();
    std::fs::File::from(std::os::fd::OwnedFd::from(fd))
        .read_to_string(&mut text)
        .expect("read");
    assert_eq!(text, KEY);
    // The positive control: the scan finds the key when it is sent as an argument on the bus
    // (an app trying to learn what a grant id looks like), so a miss above means absent, not blind.
    let _ = PeerProxy::new(&rig.stranger().await)
        .await
        .expect("proxy")
        .resolve_key(KEY)
        .await;

    let seen = tap.drain().await;
    assert!(
        seen.len() > 20,
        "the monitor saw the traffic: {}",
        seen.len()
    );
    let carrying: Vec<_> = seen.iter().filter(|m| contains(m, KEY)).collect();
    assert_eq!(
        carrying.len(),
        1,
        "only the deliberate control carries the key"
    );
    assert!(
        carrying[0].windows(10).any(|w| w == b"ResolveKey"),
        "and it is the control's call"
    );
    assert!(
        seen.iter().any(|m| contains(m, grant.as_str())),
        "positive control: the scan sees values that do cross"
    );
    // The same release in the audit: recorded, and the key is not in it.
    let audited = serde_json::to_string(&rig.audit.entries()).expect("json");
    assert!(audited.contains("key_resolved"), "{audited}");
    assert!(!audited.contains(KEY), "{audited}");
}
