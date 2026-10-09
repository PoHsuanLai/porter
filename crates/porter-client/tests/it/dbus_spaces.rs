//! The desktop-wide Spaces through the client (`DbusTransport::spaces`), against the real
//! accountd front end on a private bus: list, create, the stream of changes, and the refusals
//! read back as `SpacesError`s.
#![cfg(feature = "dbus")]

use crate::common;

use common::accountd::{Daemon, named, photos};
use porter_client::{DbusTransport, SpacesError};
use porter_core::{SpaceChange, SpaceLook, SpaceName};

fn name(text: &str) -> SpaceName {
    SpaceName::parse(text).expect("name")
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_lists_creates_and_hears_of_new_spaces() {
    let daemon = Daemon::start([]).await;
    let watcher = DbusTransport::over(daemon.client_as(named("org.quire.Mail")).await)
        .spaces()
        .await
        .expect("spaces");
    let mut changes = watcher.watch().await.expect("watch");
    assert!(watcher.list().await.expect("list").is_empty());

    let maker = DbusTransport::over(daemon.client_as(photos()).await)
        .spaces()
        .await
        .expect("spaces");
    let look = SpaceLook::parse(r#"{"colour":"teal"}"#).expect("look");
    let id = maker.create(&name("Work"), &look).await.expect("create");

    let list = watcher.list().await.expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, id);
    assert_eq!(list[0].name, name("Work"));
    assert_eq!(list[0].look, look);

    use porter_dbus::BusStream;
    let next = tokio::time::timeout(
        porter_fake::GENEROUS,
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut changes).poll_next(cx)),
    )
    .await
    .expect("told in time")
    .expect("a change")
    .expect("well formed");
    assert_eq!(next, (id, SpaceChange::Created));
}

#[tokio::test(flavor = "multi_thread")]
async fn too_many_new_spaces_and_a_stranger_are_told_apart() {
    let daemon = Daemon::start([]).await;
    let app = DbusTransport::over(daemon.client().await)
        .spaces()
        .await
        .expect("spaces");
    for n in 0..accountd::CREATES_PER_WINDOW {
        app.create(&name(&format!("Space {n}")), &SpaceLook::default())
            .await
            .expect("create");
    }
    assert_eq!(
        app.create(&name("One more"), &SpaceLook::default()).await,
        Err(SpacesError::TooMany)
    );

    let stranger = DbusTransport::over(daemon.stranger().await)
        .spaces()
        .await
        .expect("spaces");
    assert!(matches!(stranger.list().await, Err(SpacesError::Denied(_))));
}
