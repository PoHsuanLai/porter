//! The bugs the jailed accounts scenarios found, each on a private bus: a refused refresh is
//! `NeedsReauth` for the shell and Settings, the shell and Settings sign any account in again
//! with no grant, and the settings module says when its rows came or went.

use crate::common;

use common::*;
use ds_settings::live::Changes;
use porter_core::capability::{Access, Delta, Offered};
use porter_core::consent::{ConsentAnswer, GrantScope};
use porter_core::need::MailNeed;
use porter_core::sheet::SheetInput;
use porter_core::wire::Refusal;
use porter_core::{AccountState, Need};
use porter_dbus::{
    AccountProxy, CallerRole, Details, ManagerProxy, PeerProxy, Sheet, TokensProxy, need_to_dbus,
};
use porter_fake::{cloud_provider, llm_provider, mail_provider};
use std::time::Duration;
use zbus::export::futures_core::Stream;
use zbus::zvariant::ObjectPath;

/// The settings module's next `Changed`, or none within a second.
async fn next_change(changes: &mut Changes) -> Option<(String, toml::Value)> {
    tokio::time::timeout(
        Duration::from_secs(1),
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut *changes).poll_next(cx)),
    )
    .await
    .ok()
    .flatten()
    .and_then(Result::ok)
    .map(|change| (change.key.0, change.value))
}

/// Every `Changed` the module sends until a second passes quietly.
async fn all_changes(changes: &mut Changes) -> Vec<(String, toml::Value)> {
    let mut seen = Vec::new();
    while let Some(change) = next_change(changes).await {
        seen.push(change);
    }
    seen
}

fn said(text: &str) -> toml::Value {
    toml::Value::String(text.to_owned())
}

/// A rig whose mail host answers `Allow` to a sheet and whose issuer refuses every refresh.
async fn refusing_rig(allow: porter_core::AccountId) -> Rig {
    let host = SheetHost::answering(SheetInput::Answer(ConsentAnswer::Allow {
        account: allow,
        scope: GrantScope::Always,
    }));
    let providers = vec![
        cloud_provider().refusing_refresh(),
        mail_provider().refusing_refresh(),
        llm_provider(),
    ];
    Rig::start_over(Default::default(), host, providers).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_refresh_on_issue_token_leaves_the_account_needing_a_sign_in_and_says_so_once() {
    let rig = refusing_rig(storage_account().id).await;
    let (photos, grant) = grant_photos(&rig).await;
    let shell = rig
        .client_as(caller("org.quire.Shell", CallerRole::SheetHost))
        .await;
    // The shell joins the roster by calling, as it does when it starts.
    let manager = ManagerProxy::new(&shell).await.expect("proxy");
    assert!(manager.needing_reauth().await.expect("list").is_empty());
    let mut shell_hears = listen(&shell).await;
    let mut changes = settings(&rig).await.changes().await.expect("changes");

    for _ in 0..2 {
        let error = TokensProxy::new(&photos)
            .await
            .expect("proxy")
            .issue_token(grant.as_str(), "webdav")
            .await
            .expect_err("the issuer refused the credential");
        assert_eq!(error_name(&error), refusal_name(Refusal::NeedsReauth));
    }
    let state = rig
        .service
        .registry()
        .accounts
        .iter()
        .find(|a| a.id == storage_account().id)
        .map(|a| a.state);
    assert_eq!(state, Some(AccountState::NeedsReauth));
    let told = heard_names(&mut shell_hears, &["NeedsReauth"]).await;
    assert_eq!(
        told.iter().filter(|name| *name == "NeedsReauth").count(),
        1,
        "{told:?}"
    );
    assert_eq!(
        all_changes(&mut changes).await,
        [(
            "accounts.fake-storage.state".to_owned(),
            said("needs_reauth")
        )]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_refresh_on_open_authenticated_leaves_the_account_needing_a_sign_in() {
    let rig = refusing_rig(mail_account().id).await;
    let mail = rig.client("org.quire.Mail").await;
    let need = need_to_dbus(&Need::Mail(MailNeed {
        access: Access::Read,
        send: Offered::Present,
        delta: Delta::Poll,
    }));
    let mut sheet = Sheet::subscribe(&mail).await.expect("subscribe");
    let path = ManagerProxy::new(&mail)
        .await
        .expect("proxy")
        .choose(&need, "mail", "interactive", "", &sheet.options())
        .await
        .expect("choose");
    let (code, results) = sheet.response(&path).await.expect("response");
    assert_eq!(code, 0, "{results:?}");
    let grant = grant_in(&results);
    let shell = rig
        .client_as(caller("org.quire.Shell", CallerRole::SheetHost))
        .await;
    // The shell joins the roster by calling, as it does when it starts.
    let manager = ManagerProxy::new(&shell).await.expect("proxy");
    assert!(manager.needing_reauth().await.expect("list").is_empty());
    let mut shell_hears = listen(&shell).await;

    let error = TokensProxy::new(&mail)
        .await
        .expect("proxy")
        .open_authenticated(grant.as_str(), "imaps://imap.mail.invalid")
        .await
        .expect_err("the issuer refused the credential");
    assert_eq!(error_name(&error), refusal_name(Refusal::NeedsReauth));
    let state = rig
        .service
        .registry()
        .accounts
        .iter()
        .find(|a| a.id == mail_account().id)
        .map(|a| a.state);
    assert_eq!(state, Some(AccountState::NeedsReauth));
    let told = heard_names(&mut shell_hears, &["NeedsReauth"]).await;
    assert!(told.contains(&"NeedsReauth".to_owned()), "{told:?}");
}

/// A host that answers every sheet with a dismissal, so a sign-in that starts ends at once.
fn dismissing() -> SheetHost {
    SheetHost::answering(SheetInput::Dismiss)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sheet_host_signs_any_account_in_again_with_no_grant() {
    let rig = Rig::start_with(Default::default(), dismissing()).await;
    let shell = rig
        .client_as(caller("org.quire.Shell", CallerRole::SheetHost))
        .await;
    let account = AccountProxy::builder(&shell)
        .path(ObjectPath::try_from(porter_dbus::account_path(&mail_account().id)).expect("path"))
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
    // The sheet opened (the host was asked) and the answer is not "you hold no grant".
    assert_eq!(rig.host_log.calls().opened.len(), 1, "{results:?}");
    assert!(
        !format!("{results:?}").contains(&refusal_name(Refusal::UnknownGrant)),
        "{results:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_signs_any_account_in_again_with_no_grant() {
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
async fn an_app_still_needs_its_grant_to_sign_an_account_in_again() {
    let rig = Rig::start_with(Default::default(), dismissing()).await;
    let app = rig.client("org.quire.Mail").await;
    let account = AccountProxy::builder(&app)
        .path(ObjectPath::try_from(porter_dbus::account_path(&mail_account().id)).expect("path"))
        .expect("path")
        .build()
        .await
        .expect("account");
    let error = account
        .reauthenticate("", &Details::new())
        .await
        .expect_err("an app with no grant does not see the account");
    assert_eq!(error_name(&error), UNKNOWN_OBJECT);
    assert!(rig.host_log.calls().opened.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_settings_listener_hears_an_account_come_and_go() {
    let rig = Rig::start().await;
    let client = settings(&rig).await;
    let mut changes = client.changes().await.expect("changes");

    client
        .invoke(&key("accounts.fake-llm.remove"))
        .await
        .expect("removed");
    let told = all_changes(&mut changes).await;
    assert!(
        told.contains(&("accounts.fake-llm.state".to_owned(), said("removed"))),
        "{told:?}"
    );
    assert!(
        !client
            .describe()
            .await
            .expect("schema")
            .key
            .iter()
            .any(|k| k.path.0.starts_with("accounts.fake-llm.")),
        "the schema a pane reads again no longer lists it"
    );

    // A local runtime reported by inferd makes the account again.
    let inferd = rig
        .client_as(caller("org.quire.Inference", CallerRole::PorterDaemon))
        .await;
    PeerProxy::new(&inferd)
        .await
        .expect("proxy")
        .report_local("fake-llm", Vec::new(), "ok")
        .await
        .expect("reported");
    let told = all_changes(&mut changes).await;
    assert!(
        told.contains(&("accounts.fake-llm.state".to_owned(), said("ok"))),
        "{told:?}"
    );
    assert!(
        client
            .describe()
            .await
            .expect("schema")
            .key
            .iter()
            .any(|k| k.path.0 == "accounts.fake-llm.state")
    );
}
