//! Role checks (PLAN §2.1): an `Agent` is refused what acts for the person, the agent launcher is
//! not accountd's to serve, an unknown sender is `AccessDenied`.

use crate::common;

use common::*;
use porter_callers::CallerRole;
use porter_dbus::Details;
use porter_dbus::accounts::account_path;
use porter_dbus::accounts::{AccountProxy, ManagerProxy};
use porter_wire::Refusal;
use zbus::zvariant::ObjectPath;

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_is_refused_every_method_that_acts_for_the_person() {
    let rig = Rig::start().await;
    let agent = rig
        .client_as(caller("org.quire.Companion", CallerRole::Agent))
        .await;
    let denied = refusal_name(Refusal::Denied);

    let manager = ManagerProxy::new(&agent).await.expect("proxy");
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

    // Nothing was shown to anyone.
    assert!(rig.host_log.calls().opened.is_empty());
}

fn storage_account_id() -> porter_core::AccountId {
    porter_fake::storage_account().id
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_launcher_is_not_served_here_because_its_interface_is_inferds() {
    let rig = Rig::start().await;
    let launcher = rig
        .client_as(caller("org.quire.Docket", CallerRole::AgentLauncher))
        .await;
    let manager = ManagerProxy::new(&launcher).await.expect("proxy");
    let add = manager
        .add_account("", "", &Details::new())
        .await
        .expect_err("add");
    assert_eq!(error_name(&add), ACCESS_DENIED);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_sender_is_access_denied_everywhere() {
    let rig = Rig::start().await;
    let stranger = rig.stranger().await;
    let manager = ManagerProxy::new(&stranger).await.expect("proxy");
    let add = manager
        .add_account("", "", &Details::new())
        .await
        .expect_err("add");
    assert_eq!(error_name(&add), ACCESS_DENIED);
    let needing = manager.needing_reauth().await.expect_err("needing");
    assert_eq!(error_name(&needing), ACCESS_DENIED);
}
