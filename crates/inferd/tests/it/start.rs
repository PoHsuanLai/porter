//! The daemon's start on the bus: once `org.quire.Inference1` is owned, a call is answered. zbus
//! starts a connection's object server on a task of its own at its first use, and a call that
//! arrives before that task listens is dropped without an answer; a daemon that claimed its name
//! first lost the first call after a start (the inferd tests that hung until the harness ended
//! them, each on its first call).

use crate::hosting;

use hosting::rig::{Plan, World};
use porter_dbus::InferenceProxy;
use porter_fake_servers::HeldRuntime;
use std::time::Duration;

/// How long the start may show it is waiting for the held runtime, and how long a call has to
/// leave before the runtime runs (see `HeldRuntime`). The start here includes making the world
/// (a private bus, the fake engines), so the pause is longer than the other two daemons'.
const PAUSE: Duration = Duration::from_millis(600);

#[tokio::test(flavor = "multi_thread")]
async fn a_call_made_as_soon_as_the_name_is_owned_is_answered() {
    // The object server starts on a runtime that runs nothing until the test lets it: a daemon
    // that claims its name only once its calls are taken cannot finish starting before then; one
    // that claims it first is serving, with nobody listening for its calls.
    let mut held = HeldRuntime::new();
    let dispatcher = held.handle().clone();
    let world = held
        .start(
            World::start(Plan {
                dispatcher_on: Some(dispatcher),
                ..Plan::default()
            }),
            PAUSE,
        )
        .await;
    let proxy = InferenceProxy::new(&world.client).await.expect("proxy");
    let call = tokio::spawn(async move { proxy.rescan().await });
    // The call is on its way (or answered) before the object server's runtime runs.
    held.release_once_sent(PAUSE).await;
    let answer = tokio::time::timeout(porter_fake::GENEROUS, call)
        .await
        .expect("the first call is answered")
        .expect("the call's task");
    assert!(answer.is_ok(), "{answer:?}");
    drop(world);
    held.finish();
}
