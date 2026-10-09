use crate::common::{network, start};
use porter_core::MachineOwner;
use porter_fake_servers::Daemon;
use porter_tailscale::{TailscaleError, UserId};

#[tokio::test]
async fn an_address_on_the_network_is_told_by_its_owner() {
    let (fake, api) = start("whois-found", Daemon::Running(network())).await;
    let who = api
        .whois("100.64.0.2:4000".parse().expect("addr"))
        .await
        .expect("whois");
    assert_eq!(who.node.id.as_str(), "nPI");
    assert_eq!(who.node.dns, "pi.tail1234.ts.net");
    assert_eq!(who.user.login, "ada@example.org");
    assert_eq!(who.owner(UserId(1)), MachineOwner::Mine);
    // The query carried the address, port and all, escaped.
    assert_eq!(
        fake.requests(),
        ["GET /localapi/v0/whois?addr=100.64.0.2%3A4000"]
    );
}

#[tokio::test]
async fn a_tagged_a_shared_and_another_users_computer_are_not_mine() {
    let (_fake, api) = start("whois-owners", Daemon::Running(network())).await;
    let who = |address: &str| {
        let api = api.clone();
        let address = address.parse().expect("addr");
        async move { api.whois(address).await.expect("whois") }
    };
    assert_eq!(
        who("100.64.0.4:1").await.owner(UserId(1)),
        MachineOwner::Tagged
    );
    assert_eq!(
        who("100.64.0.5:1").await.owner(UserId(1)),
        MachineOwner::Shared
    );
    // Seen from the other user, the first computer is not theirs.
    assert_eq!(
        who("100.64.0.2:1").await.owner(UserId(2)),
        MachineOwner::Shared
    );
}

#[tokio::test]
async fn this_computers_own_address_is_known_too() {
    // The serving side must be able to tell its own address from a peer's.
    let (_fake, api) = start("whois-self", Daemon::Running(network())).await;
    let who = api
        .whois("100.64.0.1:9".parse().expect("addr"))
        .await
        .expect("whois");
    assert_eq!(who.node.id.as_str(), "nSELF");
}

#[tokio::test]
async fn an_address_no_computer_has_is_no_such_peer() {
    let (_fake, api) = start("whois-none", Daemon::Running(network())).await;
    let missing = api.whois("100.64.9.9:1".parse().expect("addr")).await;
    assert_eq!(missing, Err(TailscaleError::NoSuchPeer));
}
