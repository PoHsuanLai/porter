//! Reaching one of the person's own computers, after asking who is at the address.

use super::common::{addresses, fake, free_port, network};
use porter_core::{MachineOwner, NodeId};
use porter_fake::GENEROUS;
use porter_fake_servers::Daemon;
use porter_tailnet::{DialError, Dialer};
use porter_tailscale::TailscaleError;
use std::net::SocketAddr;
use tokio::net::TcpListener;

fn node(text: &str) -> NodeId {
    NodeId::parse(text).expect("a node id")
}

#[tokio::test]
async fn a_connection_to_one_of_the_persons_computers_leaves_from_this_computers_address() {
    let a = addresses();
    let port = free_port();
    let (_fake, api) = fake("dial", Daemon::Running(network(&a))).await;
    let pi = TcpListener::bind(SocketAddr::new(a.pi, port))
        .await
        .unwrap();
    let dialer = Dialer::new(api, port);
    let pi_id = node("nPI");
    let (stream, accepted) = tokio::join!(dialer.connect(&pi_id), async {
        tokio::time::timeout(GENEROUS, pi.accept())
            .await
            .unwrap()
            .unwrap()
    });
    stream.expect("connected");
    assert_eq!(
        accepted.1.ip(),
        a.me,
        "it comes from this computer's own address on the network"
    );
}

#[tokio::test]
async fn a_computer_that_is_not_the_persons_own_or_not_there_is_not_dialled() {
    let a = addresses();
    let port = free_port();
    let (fake, api) = fake("dial-refused", Daemon::Running(network(&a))).await;
    // Something listens at every address, so only the judgement can stop a connection.
    let mut held = Vec::new();
    for ip in [a.pi, a.other, a.spare] {
        held.push(TcpListener::bind(SocketAddr::new(ip, port)).await.unwrap());
    }
    let dialer = Dialer::new(api, port);
    let error = |result: Result<_, DialError>| result.map(|_| ()).unwrap_err();
    assert_eq!(
        error(dialer.connect(&node("nBUILD")).await),
        DialError::NotYours(MachineOwner::Tagged)
    );
    assert_eq!(
        error(dialer.connect(&node("nFRIEND")).await),
        DialError::NotYours(MachineOwner::Shared)
    );
    assert_eq!(
        error(dialer.connect(&node("nOLD")).await),
        DialError::Offline
    );
    assert_eq!(
        error(dialer.connect(&node("nNOBODY")).await),
        DialError::NoSuchComputer
    );
    // The computer is tagged after it was dialled once: the next connection checks again.
    dialer.connect(&node("nPI")).await.expect("first");
    fake.edit(|net| net.peers[0].tags.push("tag:ci".into()));
    assert_eq!(
        error(dialer.connect(&node("nPI")).await),
        DialError::NotYours(MachineOwner::Tagged)
    );
    drop(held);
}

#[tokio::test]
async fn nothing_answering_and_tailscale_away_are_told_in_their_own_words() {
    let a = addresses();
    let port = free_port();
    let (fake, api) = fake("dial-away", Daemon::Running(network(&a))).await;
    let dialer = Dialer::new(api, port);
    // Nothing listens at pi's address.
    assert_eq!(
        dialer.connect(&node("nPI")).await.map(|_| ()).unwrap_err(),
        DialError::Unreachable
    );
    fake.set(Daemon::SignedOut {
        auth_url: String::new(),
    });
    assert_eq!(
        dialer.connect(&node("nPI")).await.map(|_| ()).unwrap_err(),
        DialError::Tailscale(TailscaleError::SignedOut)
    );
    fake.stop();
    assert_eq!(
        dialer.connect(&node("nPI")).await.map(|_| ()).unwrap_err(),
        DialError::Tailscale(TailscaleError::NotRunning)
    );
}
