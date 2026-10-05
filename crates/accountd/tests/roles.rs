//! Role checks (PLAN §2.1): an `Agent` is refused what acts for the person, an unknown sender is
//! `AccessDenied`, and an ordinary app is served.

mod common;

use common::*;
use porter_core::wire::Refusal;
use porter_dbus::{AccountProxy, CallerRole, Details, ManagerProxy, TokensProxy, account_path};
use zbus::zvariant::ObjectPath;

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_is_refused_every_method_that_acts_for_the_person() {
    let rig = Rig::start_with(Default::default(), allowing_host()).await;
    let agent = rig
        .client_as(caller("org.quire.Companion", CallerRole::Agent))
        .await;
    let denied = refusal_name(Refusal::Denied);

    let manager = ManagerProxy::new(&agent).await.expect("proxy");
    let choose = manager
        .choose(
            &storage_need(),
            "photos",
            "interactive",
            "",
            &Details::new(),
        )
        .await
        .expect_err("choose");
    assert_eq!(error_name(&choose), denied);
    let add = manager
        .add_account("", "", &Details::new())
        .await
        .expect_err("add");
    assert_eq!(error_name(&add), denied);

    let account = AccountProxy::builder(&agent)
        .path(ObjectPath::try_from(account_path(&storage_account_id())).expect("path"))
        .expect("path")
        .build()
        .await
        .expect("proxy");
    let reauth = account
        .reauthenticate("", &Details::new())
        .await
        .expect_err("reauthenticate");
    assert_eq!(error_name(&reauth), denied);

    let tokens = TokensProxy::new(&agent).await.expect("proxy");
    let issue = tokens.issue_token("g1", "webdav").await.expect_err("token");
    assert_eq!(error_name(&issue), denied);
    let relay = tokens
        .open_authenticated("g1", "imaps://imap.mail.invalid")
        .await
        .expect_err("relay");
    assert_eq!(error_name(&relay), denied);

    // Nothing was shown to anyone.
    assert!(rig.host_log.calls().opened.is_empty());
}

fn storage_account_id() -> porter_core::AccountId {
    porter_fake::storage_account().id
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_still_ask_what_it_may_see() {
    let rig = Rig::start().await;
    let agent = rig
        .client_as(caller("org.quire.Companion", CallerRole::Agent))
        .await;
    let manager = ManagerProxy::new(&agent).await.expect("proxy");
    let found = manager
        .query(&storage_need(), "photos", "interactive")
        .await
        .expect("query");
    assert!(found.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_sender_is_access_denied_everywhere() {
    let rig = Rig::start().await;
    let stranger = rig.stranger().await;
    let manager = ManagerProxy::new(&stranger).await.expect("proxy");
    let query = manager
        .query(&storage_need(), "photos", "interactive")
        .await
        .expect_err("query");
    assert_eq!(error_name(&query), ACCESS_DENIED);
    let adopt = manager.adopt(&Details::new()).await.expect_err("adopt");
    assert_eq!(error_name(&adopt), ACCESS_DENIED);
    let tokens = TokensProxy::new(&stranger).await.expect("proxy");
    let issue = tokens.issue_token("g", "a").await.expect_err("token");
    assert_eq!(error_name(&issue), ACCESS_DENIED);
}
