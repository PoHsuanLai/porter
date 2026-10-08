//! The daemon's start on the bus: once `org.quire.Inference1` is owned, a call is answered. zbus
//! starts a connection's object server on a task of its own at its first use, and a call that
//! arrives before that task listens is dropped without an answer; a daemon that claimed its name
//! first lost the first call after a start (the inferd tests that hung until the harness ended
//! them, each on its first call).

use crate::hosting;

use hosting::rig::{Plan, World};
use porter_dbus::InferenceProxy;
use std::time::Duration;

/// How long the object server's runtime is held back.
const HELD: Duration = Duration::from_secs(2);

#[tokio::test(flavor = "multi_thread")]
async fn a_call_made_as_soon_as_the_name_is_owned_is_answered() {
    // The object server starts on a runtime that runs nothing until the test lets it, or until
    // `HELD` has passed: a daemon that claims its name only once its calls are taken is still
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
    let world = World::start(Plan {
        dispatcher_on: Some(handle),
        ..Plan::default()
    })
    .await;
    let proxy = InferenceProxy::new(&world.client).await.expect("proxy");
    let call = tokio::spawn(async move { proxy.rescan().await });
    // The call is on its way (or answered) before the object server's runtime runs.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = release.send(());
    let answer = tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .expect("the first call is answered")
        .expect("the call's task");
    assert!(answer.is_ok(), "{answer:?}");
    let _ = stop.send(());
    drop(world);
    runner.join().expect("the held runtime's thread");
}
