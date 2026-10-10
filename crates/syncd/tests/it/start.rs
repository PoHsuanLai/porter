//! syncd's start on the bus: once `org.quire.Sync1` is owned, a call is answered. zbus starts a
//! connection's object server on a task of its own at its first use, and a call that arrives
//! before that task listens is dropped without an answer; a syncd that claimed its name first
//! lost the first call after a start (rel-13: inferd and accountd had the same window).

use crate::common;

use common::Known;
use common::bus::PrivateBus;
use porter_dbus::SYNC_BUS;
use porter_fake_servers::HeldRuntime;
use std::time::Duration;
use syncd::service::{Hub, serve};

/// How long the start may show it is waiting for the held runtime, and how long a call has to
/// leave before the runtime runs (see `HeldRuntime`).
const PAUSE: Duration = Duration::from_millis(300);

#[tokio::test(flavor = "multi_thread")]
async fn a_call_made_as_soon_as_the_name_is_owned_is_answered() {
    // The object server starts on a runtime that runs nothing until the test lets it: a syncd
    // that claims its name only once its calls are taken cannot finish starting before then; one
    // that claims it first is serving, with nobody listening for its calls.
    let mut held = HeldRuntime::new();

    let bus = PrivateBus::start();
    let connection = bus.connect().await;
    {
        let _on = held.handle().enter();
        connection.object_server();
    }
    held.start(serve(&connection, Hub::default(), Known::default()), PAUSE)
        .await
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
    held.release_once_sent(PAUSE).await;
    let answer = tokio::time::timeout(porter_fake::GENEROUS, call)
        .await
        .expect("the first call is answered within the generous wait")
        .expect("the call's task");
    assert!(answer.is_ok(), "{answer:?}");
    drop(connection);
    held.finish();
}
