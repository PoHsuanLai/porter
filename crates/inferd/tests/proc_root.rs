//! The real `inferd` binary on a private bus, naming its caller through `INFERD_PROC_ROOT`: a
//! fake `/proc` with `<dir>/<pid>/cgroup` for this test process. Only a `test-proc-root` build
//! honours the variable; any other build ignores it and says so.

#[path = "hosting/bus.rs"]
mod bus;

use porter_core::Need;
use porter_core::need::LlmNeed;
use porter_dbus::{Details, INFERENCE_BUS, InferenceProxy, need_to_dbus};
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// The probe table names no port: this daemon must not ask a port of this computer what it is
/// (a real Ollama may be on one).
const CONFIG: &str = "[callers.apps]\n\"org.quire.Companion\" = [\"companiond.service\"]\n\n[probe]\nollama = []\nllama_cpp = []\nlm_studio = []\n";
const CGROUP: &str =
    "0::/user.slice/user-1000.slice/user@1000.service/app.slice/companiond.service\n";

struct Daemon {
    child: Child,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(bus: &bus::PrivateBus, proc_root: &Path) -> Daemon {
    let scratch = bus.scratch();
    let config = scratch.join("inferd.toml");
    std::fs::write(&config, CONFIG).expect("config");
    let child = Command::new(env!("CARGO_BIN_EXE_inferd"))
        .env_clear()
        .env("HOME", scratch)
        .env("XDG_RUNTIME_DIR", scratch)
        .env("XDG_CONFIG_HOME", scratch.join("config"))
        .env("XDG_DATA_HOME", scratch.join("data"))
        .env("XDG_STATE_HOME", scratch.join("state"))
        .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
        .env("INFERD_PROC_ROOT", proc_root)
        .arg("--config")
        .arg(&config)
        .stderr(Stdio::null())
        .spawn()
        .expect("inferd binary");
    Daemon { child }
}

/// Whether `availability` from this process is answered (a named caller) or refused.
async fn answered(bus: &bus::PrivateBus, proc_root: &Path) -> bool {
    let client = bus.connect().await;
    let dbus = zbus::fdo::DBusProxy::new(&client).await.expect("dbus");
    let name = zbus::names::BusName::try_from(INFERENCE_BUS).expect("bus name");
    let _daemon = start(bus, proc_root);
    // The daemon's claim is the event; asking again is the only way to see it without a stream.
    while !dbus.name_has_owner(name.clone()).await.expect("ask") {
        tokio::task::yield_now().await;
    }
    let proxy = InferenceProxy::new(&client).await.expect("proxy");
    let need = need_to_dbus(&Need::Llm(LlmNeed {
        features: Default::default(),
        context: porter_core::Tokens(1),
    }));
    proxy
        .availability(&need, "notes", &Details::new())
        .await
        .is_ok()
}

fn fake_proc(root: &Path, pid: u32, cgroup: Option<&str>) {
    let dir = root.join(pid.to_string());
    std::fs::create_dir_all(&dir).expect("pid dir");
    if let Some(text) = cgroup {
        std::fs::write(dir.join("cgroup"), text).expect("cgroup");
    }
}

#[cfg(feature = "test-proc-root")]
#[tokio::test(flavor = "multi_thread")]
async fn a_caller_in_the_fake_proc_root_is_named_and_one_outside_it_is_not() {
    let named = bus::PrivateBus::start();
    let root = named.scratch().join("proc");
    fake_proc(&root, std::process::id(), Some(CGROUP));
    assert!(answered(&named, &root).await);

    let nobody = bus::PrivateBus::start();
    let root = nobody.scratch().join("proc");
    fake_proc(&root, std::process::id(), None);
    assert!(!answered(&nobody, &root).await);
}

#[cfg(not(feature = "test-proc-root"))]
#[tokio::test(flavor = "multi_thread")]
async fn without_the_feature_the_variable_is_ignored() {
    let bus = bus::PrivateBus::start();
    let root = bus.scratch().join("proc");
    fake_proc(&root, std::process::id(), Some(CGROUP));
    assert!(!answered(&bus, &root).await);
}
