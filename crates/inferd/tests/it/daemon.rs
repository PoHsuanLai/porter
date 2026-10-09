//! inferd run from a `Config` inside the test process: no environment variable is read (the
//! directories come from a map the test passes, the bus from the private bus's address, the
//! Tailscale socket from the config), the name is owned and answers once `Daemon::build` has
//! returned, and `run` ends and lets the name go when its stop is asked for.

use crate::hosting::bus::{self, PrivateBus};

use inferd::config::Dirs;
use inferd::{Config, Daemon};
use porter_dbus::{BusTarget, INFERENCE_BUS};
use porter_fake::Deadline;
use std::path::Path;
use std::time::Duration;

/// The probe table names no port: this daemon must not ask a port of this computer what it is
/// (a real Ollama may be on one).
const FILE: &str = "[probe]\nollama = []\nllama_cpp = []\nlm_studio = []\n";

/// The directories for a person whose whole home is `home`; every file is under it.
fn dirs(home: &Path) -> Dirs {
    let at = |sub: &str| home.join(sub).to_string_lossy().into_owned();
    let map = [
        ("HOME", at("")),
        ("XDG_RUNTIME_DIR", at("run")),
        ("XDG_CONFIG_HOME", at("config")),
        ("XDG_DATA_HOME", at("data")),
        ("XDG_STATE_HOME", at("state")),
        ("HF_HOME", at("hf")),
    ];
    let mut dirs = Dirs::from_vars(|name| {
        map.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
    })
    .expect("dirs");
    // Not this computer's own catalog.
    dirs.catalog.system = home.join("no-catalog");
    dirs
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_built_from_a_config_serves_its_name_and_stops_when_told() {
    let bus = PrivateBus::start();
    let home = bus.scratch().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let file = home.join("inferd.toml");
    std::fs::write(&file, FILE).expect("config file");
    let config = Config::new(dirs(&home), BusTarget::Address(bus.address().to_owned()))
        .with_config_file(file)
        // A Tailscale nobody serves: this computer's real one is never asked.
        .with_tailscale_socket(home.join("no-tailscale.sock"));

    let daemon = bus::within("inferd to start", Daemon::build(config))
        .await
        .expect("inferd starts");

    let probe = bus.connect().await;
    let dbus = zbus::fdo::DBusProxy::new(&probe).await.expect("proxy");
    let name = zbus::names::BusName::try_from(INFERENCE_BUS).expect("name");
    assert!(
        dbus.name_has_owner(name.clone()).await.expect("ask"),
        "the name is owned once build has returned"
    );
    bus::within(
        "inferd to answer a ping",
        probe.call_method(
            Some(INFERENCE_BUS),
            "/",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        ),
    )
    .await
    .expect("a call is answered");

    let (stop, stopped) = tokio::sync::mpsc::unbounded_channel::<()>();
    let running = tokio::spawn(daemon.run(stopped));
    // `run` does nothing until it is told.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!running.is_finished(), "run waits for its stop");
    stop.send(()).expect("run is waiting");
    bus::within("inferd to stop", running)
        .await
        .expect("run's task")
        .expect("a clean stop");

    let deadline = Deadline::generous();
    while dbus.name_has_owner(name.clone()).await.expect("ask") {
        if deadline.passed() {
            deadline.fail("inferd lets go of its name");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
