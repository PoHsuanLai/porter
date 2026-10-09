use crate::common::{LOGIN_PAGE, network, start};
use porter_fake_servers::Daemon;
use porter_tailscale::{Backend, Notice, TailscaleError, Watch};
use std::time::Duration;

async fn next(watch: &mut Watch) -> Notice {
    tokio::time::timeout(Duration::from_secs(5), watch.next())
        .await
        .expect("a notice in time")
        .expect("a notice")
        .expect("the stream is open")
}

#[tokio::test]
async fn the_watch_starts_with_the_state_and_follows_the_changes() {
    let (fake, api) = start("watch-follows", Daemon::Running(network())).await;
    let mut watch = api.watch().await.expect("watch");
    let first = next(&mut watch).await;
    assert_eq!(first.state, Some(Backend::Running));
    assert!(next(&mut watch).await.net_map, "and the network");
    assert_eq!(fake.watchers(), 1);

    fake.edit(|net| net.peers[0].online = false);
    assert!(next(&mut watch).await.net_map);

    fake.set(Daemon::SignedOut {
        auth_url: LOGIN_PAGE.to_owned(),
    });
    assert_eq!(next(&mut watch).await.state, Some(Backend::NeedsLogin));
}

#[tokio::test]
async fn a_sign_in_asked_for_sends_the_page_on_the_watch() {
    let (_fake, api) = start(
        "watch-login",
        Daemon::SignedOut {
            auth_url: LOGIN_PAGE.to_owned(),
        },
    )
    .await;
    let mut watch = api.watch().await.expect("watch");
    assert_eq!(next(&mut watch).await.state, Some(Backend::NeedsLogin));
    api.start_login().await.expect("login");
    assert_eq!(
        next(&mut watch).await.browse_to.as_deref(),
        Some(LOGIN_PAGE)
    );
}

#[tokio::test]
async fn the_watch_ends_when_tailscale_stops() {
    let (fake, api) = start("watch-stops", Daemon::Running(network())).await;
    let mut watch = api.watch().await.expect("watch");
    next(&mut watch).await;
    next(&mut watch).await;
    fake.stop();
    let end = tokio::time::timeout(Duration::from_secs(5), watch.next())
        .await
        .expect("it ends");
    // Closed cleanly, or cut: either way the caller is told and goes to ask again.
    assert!(
        matches!(end, Ok(None) | Err(TailscaleError::NotRunning)),
        "{end:?}"
    );
}

#[tokio::test]
async fn dropping_the_watch_closes_the_stream() {
    let (fake, api) = start("watch-dropped", Daemon::Running(network())).await;
    let mut watch = api.watch().await.expect("watch");
    next(&mut watch).await;
    assert_eq!(fake.watchers(), 1);
    drop(watch);
    for _ in 0..100 {
        if fake.watchers() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the stream stayed open");
}
