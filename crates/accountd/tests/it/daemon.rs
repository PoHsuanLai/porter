//! accountd run from a `Config` inside the test process: no environment variable is read (the
//! paths come from a map the test passes, the bus from the private bus's address), the name is
//! owned and answers once `Daemon::build` has returned, and `run` ends and lets the name go when
//! its shutdown completes.

use crate::common::bus::PrivateBus;

use accountd::paths::Paths;
use accountd::{Config, Daemon};
use porter_dbus::{ACCOUNTS_BUS, BusTarget};
use porter_fake::{Deadline, GENEROUS};
use std::path::Path;
use std::time::Duration;

/// The paths for a person whose whole home is `home`; every file is under it.
fn paths(home: &Path) -> Paths {
    let at = |sub: &str| home.join(sub).to_string_lossy().into_owned();
    let map = [
        ("HOME", at("")),
        ("XDG_STATE_HOME", at("state")),
        ("XDG_CONFIG_HOME", at("config")),
        ("XDG_DATA_HOME", at("data")),
        ("XDG_DATA_DIRS", at("share")),
        // Never the real Tailscale: a socket nobody serves.
        (
            accountd::paths::TAILSCALE_SOCKET_VAR,
            at("no-tailscale.sock"),
        ),
    ];
    let mut paths = Paths::resolve(
        |name| {
            map.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        },
        &[],
    )
    .expect("paths");
    // Nothing of this computer's own: not its caller table, its shipped clients or its providers.
    paths.callers_system = home.join("no-callers.toml");
    paths.clients_shipped = home.join("no-clients.toml");
    paths.provider_dirs = vec![
        home.join("providers-shipped"),
        home.join("providers-person"),
    ];
    paths
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_built_from_a_config_serves_its_name_and_stops_when_told() {
    let bus = PrivateBus::start();
    let home = bus.scratch().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let config = Config::new(paths(&home), BusTarget::Address(bus.address().to_owned()));

    let daemon = tokio::time::timeout(GENEROUS, Daemon::build(config))
        .await
        .expect("accountd starts within the generous wait")
        .expect("accountd starts");

    let probe = bus.connect().await;
    let dbus = zbus::fdo::DBusProxy::new(&probe).await.expect("proxy");
    let name = zbus::names::BusName::try_from(ACCOUNTS_BUS).expect("name");
    assert!(
        dbus.name_has_owner(name.clone()).await.expect("ask"),
        "the name is owned once build has returned"
    );
    tokio::time::timeout(
        GENEROUS,
        probe.call_method(
            Some(ACCOUNTS_BUS),
            "/",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        ),
    )
    .await
    .expect("answered within the generous wait")
    .expect("a call is answered");

    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let running = tokio::spawn(daemon.run(async {
        let _ = stopped.await;
    }));
    // Still serving: `run` does nothing until it is told.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!running.is_finished(), "run waits for its shutdown");
    stop.send(()).expect("run is waiting");
    tokio::time::timeout(GENEROUS, running)
        .await
        .expect("run ends within the generous wait")
        .expect("run's task");

    let deadline = Deadline::generous();
    while dbus.name_has_owner(name.clone()).await.expect("ask") {
        if deadline.passed() {
            deadline.fail("accountd lets go of its name");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
