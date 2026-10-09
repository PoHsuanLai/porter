//! Following Tailscale: who this computer is, while Tailscale is running and signed in.

use super::common::{addresses, fake, network, observed, wait};
use porter_fake_servers::Daemon;

#[tokio::test]
async fn this_computer_is_seen_while_tailscale_is_running_and_signed_in() {
    let a = addresses();
    let (fake, api) = fake("observe", Daemon::Running(network(&a))).await;
    let (observer, mut seen) = observed(&api).await;
    let me = seen.borrow().clone().expect("this computer");
    assert_eq!(me.node().as_str(), "nSELF");
    assert_eq!(me.name(), "desk");
    assert_eq!(me.addresses(), [a.me]);

    // A new address is noticed from the stream.
    fake.edit(|net| net.me.addresses = vec![a.spare.to_string()]);
    wait(
        &mut seen,
        |now| now.as_ref().is_some_and(|me| me.addresses() == [a.spare]),
        "the new address",
    )
    .await;

    // Signed out: not seen. Signed in again: seen.
    fake.set(Daemon::SignedOut {
        auth_url: "https://login.tailscale.com/a/x".into(),
    });
    wait(&mut seen, Option::is_none, "to be signed out").await;
    fake.set(Daemon::Running(network(&a)));
    wait(&mut seen, Option::is_some, "to be signed in again").await;

    // Not running (killed): not seen. Running again: seen.
    fake.stop();
    wait(&mut seen, Option::is_none, "to be gone").await;
    fake.restart().await.unwrap();
    wait(&mut seen, Option::is_some, "to be back").await;
    drop(observer);
}

#[tokio::test]
async fn a_change_the_stream_did_not_announce_is_found_by_the_look_behind_it() {
    let a = addresses();
    let (fake, api) = fake("observe-quiet", Daemon::Running(network(&a))).await;
    let (_observer, mut seen) = observed(&api).await;
    fake.edit_quietly(|net| net.me.addresses = vec![a.spare.to_string()]);
    wait(
        &mut seen,
        |now| now.as_ref().is_some_and(|me| me.addresses() == [a.spare]),
        "the look to find it",
    )
    .await;
}

#[tokio::test]
async fn a_tailscale_that_is_starting_changes_nothing() {
    let a = addresses();
    let (fake, api) = fake("observe-starting", Daemon::Running(network(&a))).await;
    let (_observer, seen) = observed(&api).await;
    fake.set(Daemon::Changing);
    // Starting settles nothing: what was known stays (the look is made again soon).
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert!(seen.borrow().is_some(), "still this computer");
}
