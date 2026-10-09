use crate::common::{LOGIN_PAGE, network, start};
use porter_core::MachineOwner;
use porter_fake_servers::Daemon;
use porter_tailscale::{Backend, Standing};

#[tokio::test]
async fn a_running_tailscale_lists_the_other_computers() {
    let (_fake, api) = start("status-running", Daemon::Running(network())).await;
    let status = api.status().await.expect("status");
    assert_eq!(status.backend, Backend::Running);
    assert_eq!(status.standing(), Standing::Ready);
    assert_eq!(status.label().as_deref(), Some("ada@example.org"));
    assert_eq!(status.me.as_ref().map(|me| me.name()), Some("desk"));
    let machines = status.machines();
    let owners: Vec<(&str, MachineOwner)> = machines
        .iter()
        .map(|m| (m.name.as_str(), m.owner))
        .collect();
    assert_eq!(
        owners,
        [
            ("build-box", MachineOwner::Tagged),
            ("friends-pc", MachineOwner::Shared),
            ("old-laptop", MachineOwner::Mine),
            ("pi", MachineOwner::Mine),
        ]
    );
    let pi = machines.iter().find(|m| m.name == "pi").expect("pi");
    assert!(pi.ssh && pi.online && pi.last_seen.is_none());
    assert_eq!(pi.dns, "pi.tail1234.ts.net");
    let old = machines
        .iter()
        .find(|m| m.name == "old-laptop")
        .expect("old");
    assert!(!old.online && !old.ssh);
    assert_eq!(old.last_seen.map(|t| t.0), Some(1_790_000_000));
}

#[tokio::test]
async fn a_renamed_computer_keeps_its_stable_node_id() {
    let (fake, api) = start("status-renamed", Daemon::Running(network())).await;
    let id_of = |status: &porter_tailscale::Status, name: &str| {
        status
            .peers
            .iter()
            .find(|p| p.name() == name)
            .map(|p| p.id.as_str().to_owned())
    };
    let before = api.status().await.expect("status");
    assert_eq!(id_of(&before, "pi").as_deref(), Some("nPI"));
    fake.edit(|net| net.peers[0].name = "pi-kitchen".into());
    let after = api.status().await.expect("status");
    assert_eq!(id_of(&after, "pi"), None);
    assert_eq!(id_of(&after, "pi-kitchen").as_deref(), Some("nPI"));
    // The whois of its address says the same id.
    let who = api
        .whois("100.64.0.2:1".parse().expect("addr"))
        .await
        .expect("whois");
    assert_eq!(who.node.id.as_str(), "nPI");
}

#[tokio::test]
async fn a_signed_out_tailscale_answers_and_gives_its_page_only_after_a_sign_in_is_asked() {
    let (fake, api) = start(
        "status-signed-out",
        Daemon::SignedOut {
            auth_url: LOGIN_PAGE.to_owned(),
        },
    )
    .await;
    let before = api.status().await.expect("status");
    assert_eq!(before.standing(), Standing::SignedOut);
    assert_eq!(before.auth_url, None);
    assert!(before.machines().is_empty());
    api.start_login().await.expect("login");
    assert_eq!(fake.logins(), 1);
    let after = api.status().await.expect("status");
    assert_eq!(after.auth_url.as_deref(), Some(LOGIN_PAGE));
}

#[tokio::test]
async fn a_tailscale_in_the_middle_of_changing_says_so() {
    let (_fake, api) = start("status-changing", Daemon::Changing).await;
    let status = api.status().await.expect("status");
    assert_eq!(status.backend, Backend::Starting);
    assert_eq!(status.standing(), Standing::Changing);
    assert!(status.machines().is_empty());
}

#[tokio::test]
async fn the_client_sends_the_host_tailscale_expects_and_nothing_to_alarm_it() {
    // The fake refuses a wrong Host, an Origin and a Referer as the real one does; the client's
    // status passing is the proof that it sends none of them.
    let (fake, api) = start("status-host", Daemon::Running(network())).await;
    api.status().await.expect("status");
    assert_eq!(fake.requests(), ["GET /localapi/v0/status"]);
}
