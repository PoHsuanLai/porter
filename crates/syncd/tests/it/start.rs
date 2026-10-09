//! syncd's start on the bus: once `org.quire.Sync1` is owned, a call is answered. zbus starts a
//! connection's object server on a task of its own at its first use, and a call that arrives
//! before that task listens is dropped without an answer; a syncd that claimed its name first
//! lost the first call after a start (rel-13: inferd and accountd had the same window).

use crate::common;

use common::Known;
use common::bus::PrivateBus;
use porter_dbus::SYNC_BUS;
use std::time::Duration;
use syncd::service::{Hub, serve};

/// How long the object server's runtime is held back.
const HELD: Duration = Duration::from_secs(2);

/// How long the test waits on anything: as long as a starved machine needs.
const DEADLINE: Duration = porter_fake::GENEROUS;

#[tokio::test(flavor = "multi_thread")]
async fn a_call_made_as_soon_as_the_name_is_owned_is_answered() {
    // The object server starts on a runtime that runs nothing until the test lets it, or until
    // `HELD` has passed: a syncd that claims its name only once its calls are taken is still
    // starting then; one that claims it first is serving, with nobody listening for its calls.
    let held = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let handle = held.handle().clone();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let runner = std::thread::spawn(move || {
        let _ = released.recv_timeout(HELD);
        held.block_on(async {
            let _ = stopped.await;
        });
    });

    let bus = PrivateBus::start();
    let connection = bus.connect().await;
    {
        let _on = handle.enter();
        connection.object_server();
    }
    tokio::time::timeout(
        DEADLINE,
        serve(&connection, Hub::default(), Known::default()),
    )
    .await
    .expect("syncd serves within the generous wait")
    .expect("syncd serves");

    let client = bus.connect().await;
    let call = tokio::spawn(async move {
        client
            .call_method(
                Some(SYNC_BUS),
                "/",
                Some("org.freedesktop.DBus.Peer"),
                "Ping",
                &(),
            )
            .await
            .map(|_| ())
    });
    // The call is on its way (or answered) before the object server's runtime runs.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = release.send(());
    let answer = tokio::time::timeout(porter_fake::GENEROUS, call)
        .await
        .expect("the first call is answered within the generous wait")
        .expect("the call's task");
    assert!(answer.is_ok(), "{answer:?}");
    let _ = stop.send(());
    drop(connection);
    runner.join().expect("the held runtime's thread");
}
