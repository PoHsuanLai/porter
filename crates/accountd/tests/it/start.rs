//! accountd's start on the bus: once `org.quire.Accounts1` is owned, a call is answered. zbus
//! starts a connection's object server on a task of its own at its first use, and a call that
//! arrives before that task listens is dropped without an answer; an accountd that claimed its
//! name first lost the first call after a start (a test that hung until the harness ended it).

use crate::common;

use accountd::{BusSheets, Options, TableCallers, serve_with};
use common::bus::PrivateBus;
use common::{Shared, caller};
use porter_dbus::{ACCOUNTS_BUS, CallerRole};
use porter_fake::{FixedClock, MemoryStore, RecordingAudit, cloud_provider};
use porter_service::{AccountService, Registry};
use std::sync::Arc;
use std::time::Duration;

/// How long the object server's runtime is held back.
const HELD: Duration = Duration::from_secs(2);

#[tokio::test(flavor = "multi_thread")]
async fn a_call_made_as_soon_as_the_name_is_owned_is_answered() {
    // The object server starts on a runtime that runs nothing until the test lets it, or until
    // `HELD` has passed: an accountd that claims its name only once its calls are taken is still
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
    let callers = Arc::new(TableCallers::new());
    let service = Arc::new(
        AccountService::new(
            vec![cloud_provider()],
            Registry {
                accounts: vec![],
                grants: vec![],
                toggles: vec![],
            },
            Shared::default(),
            BusSheets::new(connection.clone(), Arc::clone(&callers)),
            FixedClock(porter_fake::NOW),
        )
        .with_store(MemoryStore::default())
        .with_audit(RecordingAudit::default()),
    );
    let served = tokio::time::timeout(
        porter_fake::GENEROUS,
        serve_with(
            &connection,
            service,
            Arc::clone(&callers),
            Options::default(),
        ),
    )
    .await
    .expect("accountd serves within the time");
    served.expect("accountd serves");

    let client = bus.connect().await;
    callers.introduce_as(
        client.unique_name().expect("name").as_str(),
        caller("org.example.App", CallerRole::App),
    );
    let call = tokio::spawn(async move {
        client
            .call_method(
                Some(ACCOUNTS_BUS),
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
        .expect("the first call is answered")
        .expect("the call's task");
    assert!(answer.is_ok(), "{answer:?}");
    let _ = stop.send(());
    drop(connection);
    runner.join().expect("the held runtime's thread");
}
