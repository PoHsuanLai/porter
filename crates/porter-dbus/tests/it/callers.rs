//! `ProcCallers` over fixture `/proc` trees (the pure resolution) and a
//! private bus where an unidentified sender is refused. Nothing reads the real `/proc` or
//! `/etc`: the roots and the table paths are the test's own.

#[path = "callers/bus.rs"]
mod bus;

use bus::PrivateBus;
use porter_core::Isolation;
use porter_dbus::{Caller, CallerRole, CallerTable, Callers, MainPids, ProcCallers};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

const SLICE: &str = "/user.slice/user-1000.slice/user@1000.service/app.slice";

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A scratch directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "callers-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Process `pid` with this cgroup line (none: no file).
    fn process(&self, pid: u32, cgroup: Option<&str>) {
        let dir = self.0.join(pid.to_string());
        std::fs::create_dir_all(&dir).expect("proc dir");
        if let Some(cgroup) = cgroup {
            std::fs::write(dir.join("cgroup"), format!("0::{cgroup}\n")).expect("cgroup");
        }
    }

    /// Process `pid` runs in a Flatpak sandbox whose metadata names `app`.
    fn flatpak(&self, pid: u32, app: &str) {
        let root = self.0.join(pid.to_string()).join("root");
        std::fs::create_dir_all(&root).expect("root");
        let info = format!("[Application]\nname={app}\n\n[Instance]\ninstance-id={pid}\n");
        std::fs::write(root.join(".flatpak-info"), info).expect("flatpak-info");
    }

    /// Process `pid` is the main process of the service `unit`.
    fn main_of(&self, unit: &str, pid: u32) {
        let units = self.0.join("units");
        std::fs::create_dir_all(&units).expect("units");
        std::fs::write(units.join(unit), format!("{pid}\n")).expect("main pid");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn table() -> CallerTable {
    serde_json::from_value(serde_json::json!({"caller": [
        {"app": "org.quire.Settings", "role": "settings"},
        {"app": "org.quire.Intent", "unit": "intentd.service", "role": "agent"},
        {"app": "org.quire.Cua", "unit": "cuad.service", "role": "cua"},
        {"app": "org.quire.Sill", "role": "sheet_host"},
        {"app": "org.quire.Inference", "unit": "inferd.service", "role": "porter_daemon"},
        {"app": "org.example.Sheets", "role": "sheet_host"},
        {"app": "org.quire.Shell", "unit": "sill-shell.scope", "role": "sheet_host"},
        {"app": "org.quire.Trap", "unit": "app-org.quire.Trap-1.scope", "role": "settings"},
    ]}))
    .expect("table")
}

fn scope(unit: &str) -> String {
    format!("{SLICE}/{unit}")
}

fn resolve(scratch: &Scratch, pid: u32) -> Option<(String, Isolation, CallerRole)> {
    ProcCallers::caller_of_pid(scratch.path(), pid, &table())
        .map(|c| (c.app.name.to_string(), c.app.isolation, c.role))
}

fn got(
    name: &str,
    isolation: Isolation,
    role: CallerRole,
) -> Option<(String, Isolation, CallerRole)> {
    Some((name.to_owned(), isolation, role))
}

#[test]
fn a_process_is_named_by_its_scope_or_unit() {
    let s = Scratch::new();
    s.process(10, Some(&scope("app-org.example.Mail-4242.scope")));
    s.process(11, Some(&scope("app-gnome-org.example.Calc-77.scope")));
    s.process(12, Some(&scope("app-flatpak-org.example.Photos-5.scope")));
    s.process(13, Some(&scope("app-flatpak-org.example.Sheets-6.scope")));
    s.process(14, Some(&scope("app-org.quire.Settings-9.scope")));
    s.process(15, Some(&scope("app-org.example.Mail\\x2dbeta-9.scope")));
    s.process(
        16,
        Some("/user.slice/user@1000.service/app.slice/inferd.service"),
    );
    s.process(17, Some("/system.slice/cuad.service"));
    s.process(
        18,
        Some("/user.slice/user@1000.service/app.slice/intentd.service"),
    );
    s.flatpak(12, "org.example.Photos");
    s.flatpak(13, "org.example.Sheets");
    s.main_of("inferd.service", 16);
    s.main_of("cuad.service", 17);
    s.main_of("intentd.service", 18);
    let cases = [
        (
            10,
            got("org.example.Mail", Isolation::Unsandboxed, CallerRole::App),
        ),
        (
            11,
            got("org.example.Calc", Isolation::Unsandboxed, CallerRole::App),
        ),
        (
            12,
            got("org.example.Photos", Isolation::Flatpak, CallerRole::App),
        ),
        // A table row gives an app its role, sandboxed or not.
        (
            13,
            got(
                "org.example.Sheets",
                Isolation::Flatpak,
                CallerRole::SheetHost,
            ),
        ),
        (
            14,
            got(
                "org.quire.Settings",
                Isolation::Unsandboxed,
                CallerRole::Settings,
            ),
        ),
        (
            15,
            got(
                "org.example.Mail-beta",
                Isolation::Unsandboxed,
                CallerRole::App,
            ),
        ),
        (
            16,
            got(
                "org.quire.Inference",
                Isolation::Unsandboxed,
                CallerRole::PorterDaemon,
            ),
        ),
        (
            17,
            got("org.quire.Cua", Isolation::Unsandboxed, CallerRole::Cua),
        ),
        (
            18,
            got(
                "org.quire.Intent",
                Isolation::Unsandboxed,
                CallerRole::Agent,
            ),
        ),
    ];
    for (pid, want) in cases {
        assert_eq!(resolve(&s, pid), want, "pid {pid}");
    }
}

#[test]
fn a_scope_named_after_a_daemons_app_is_an_app() {
    // Any process may start a scope by any name: only the unit itself carries the unit's role.
    let s = Scratch::new();
    s.process(30, Some(&scope("app-org.quire.Cua-1.scope")));
    s.process(31, Some(&scope("app-flatpak-org.quire.Inference-2.scope")));
    s.process(32, Some(&scope("app-org.quire.Intent-3.scope")));
    s.flatpak(31, "org.quire.Inference");
    let cases = [
        (
            30,
            got("org.quire.Cua", Isolation::Unsandboxed, CallerRole::App),
        ),
        (
            31,
            got("org.quire.Inference", Isolation::Flatpak, CallerRole::App),
        ),
        (
            32,
            got("org.quire.Intent", Isolation::Unsandboxed, CallerRole::App),
        ),
    ];
    for (pid, want) in cases {
        assert_eq!(resolve(&s, pid), want, "pid {pid}");
    }
}

#[test]
fn a_scope_a_row_names_as_its_unit_is_that_unit_exactly() {
    let s = Scratch::new();
    s.process(40, Some(&scope("sill-shell.scope")));
    s.process(41, Some(&scope("sill-shell-2.scope")));
    s.process(42, Some(&scope("xsill-shell.scope")));
    // An app scope never takes a unit row's role, even one naming it exactly.
    s.process(43, Some(&scope("app-org.quire.Trap-1.scope")));
    assert_eq!(
        resolve(&s, 40),
        got(
            "org.quire.Shell",
            Isolation::Unsandboxed,
            CallerRole::SheetHost
        )
    );
    assert_eq!(resolve(&s, 41), None);
    assert_eq!(resolve(&s, 42), None);
    assert_eq!(
        resolve(&s, 43),
        got("org.quire.Trap", Isolation::Unsandboxed, CallerRole::App)
    );
}

#[test]
fn a_process_nothing_names_is_nobody() {
    let s = Scratch::new();
    // A session scope; a terminal's child (also a program started by hand); a launcher scope
    // that is not an id; a unit the table does not list; a Flatpak scope with no instance;
    // no cgroup file at all; a cgroup v1 file.
    s.process(20, Some("/user.slice/user-1000.slice/session-2.scope"));
    s.process(21, Some(&scope("vte-spawn-1f2e.scope")));
    s.process(22, Some(&scope("app-firefox-3.scope")));
    s.process(24, Some("/system.slice/other.service"));
    s.process(26, Some(&scope("app-flatpak-org.example.Y.scope")));
    s.process(27, None);
    let dir = s.path().join("28");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("cgroup"), "1:name=systemd:/user.slice\n").expect("v1");
    for pid in [20, 21, 22, 24, 25, 26, 27, 28] {
        assert_eq!(resolve(&s, pid), None, "pid {pid}");
    }
}

#[test]
fn an_unreadable_root_is_no_sandbox_and_a_flatpak_scope_then_proves_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new();
    s.process(50, Some(&scope("app-flatpak-org.example.Photos-5.scope")));
    s.process(51, Some("/system.slice/inferd.service"));
    s.process(52, Some(&scope("app-org.example.Mail-4242.scope")));
    s.main_of("inferd.service", 51);
    // `exe` and `root` are there and unreadable, as ptrace-gated entries are under Landlock;
    // `exe` is never read.
    for pid in [50, 51, 52] {
        let dir = s.path().join(pid.to_string());
        for entry in ["exe", "root"] {
            std::fs::create_dir(dir.join(entry)).expect("entry");
            std::fs::set_permissions(dir.join(entry), std::fs::Permissions::from_mode(0o0))
                .expect("chmod");
        }
    }
    // The scope's name alone is no proof of a Flatpak app (sec-3 c).
    assert_eq!(resolve(&s, 50), None);
    assert_eq!(
        resolve(&s, 51),
        got(
            "org.quire.Inference",
            Isolation::Unsandboxed,
            CallerRole::PorterDaemon
        )
    );
    assert_eq!(
        resolve(&s, 52),
        got("org.example.Mail", Isolation::Unsandboxed, CallerRole::App)
    );
    for pid in [50, 51, 52] {
        for entry in ["exe", "root"] {
            let path = s.path().join(pid.to_string()).join(entry);
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
}

#[test]
fn a_flatpak_app_is_named_by_its_metadata_and_never_by_its_scope() {
    let s = Scratch::new();
    // A Flatpak scope with no metadata: any program can start a scope by that name.
    s.process(60, Some(&scope("app-flatpak-org.example.Photos-5.scope")));
    // Metadata in another app's native scope, or in a daemon's unit as its main process: the
    // sandbox's word wins, and a sandboxed process gets no row's unit or app role.
    s.process(61, Some(&scope("app-org.quire.Settings-1.scope")));
    s.flatpak(61, "org.example.Photos");
    s.process(62, Some("/system.slice/inferd.service"));
    s.flatpak(62, "org.example.Photos");
    s.main_of("inferd.service", 62);
    // Metadata that is there and names no application: nobody, in any scope.
    s.process(63, Some(&scope("app-org.example.Mail-1.scope")));
    let root = s.path().join("63/root");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join(".flatpak-info"), "[Instance]\ninstance-id=1\n").expect("info");
    let photos = got("org.example.Photos", Isolation::Flatpak, CallerRole::App);
    assert_eq!(resolve(&s, 60), None);
    assert_eq!(resolve(&s, 61), photos);
    assert_eq!(resolve(&s, 62), photos);
    assert_eq!(resolve(&s, 63), None);
}

#[test]
fn a_service_role_goes_only_to_the_units_main_process() {
    let s = Scratch::new();
    let unit = "/user.slice/user@1000.service/app.slice/inferd.service";
    s.process(70, Some(unit));
    s.main_of("inferd.service", 70);
    // Another process of the person's that moved itself into the unit's cgroup (sec-3 b).
    s.process(71, Some(unit));
    // A unit whose main process nothing states (not running, or no manager to ask).
    s.process(72, Some("/system.slice/cuad.service"));
    assert_eq!(
        resolve(&s, 70),
        got(
            "org.quire.Inference",
            Isolation::Unsandboxed,
            CallerRole::PorterDaemon
        )
    );
    assert_eq!(resolve(&s, 71), None);
    assert_eq!(resolve(&s, 72), None);
}

#[tokio::test]
async fn on_a_private_bus_an_unidentified_sender_is_refused_and_an_identified_one_is_named() {
    let s = Scratch::new();
    let bus = PrivateBus::start();
    let client = bus.connect().await;
    let sender = client.unique_name().expect("unique name").to_string();
    let service = bus.connect().await;
    let pid = std::process::id();

    // The test process has no app scope in this tree: nobody.
    s.process(pid, Some("/user.slice/session-2.scope"));
    let callers = ProcCallers::with_proc_root(service.clone(), table(), s.path().to_owned());
    assert_eq!(callers.caller_of(&sender).await, None);
    // A name that is not on the bus, and text that is not a name: nobody.
    assert_eq!(callers.caller_of(":1.9999").await, None);
    assert_eq!(callers.caller_of("not a name").await, None);

    // The same process, now in an app's scope.
    s.process(pid, Some(&scope("app-org.example.Mail-4242.scope")));
    let caller = callers.caller_of(&sender).await.expect("identified");
    assert_eq!(
        (caller.app.name.as_str(), caller.app.isolation, caller.role),
        ("org.example.Mail", Isolation::Unsandboxed, CallerRole::App)
    );

    // And as a listed app.
    s.process(pid, Some(&scope("app-flatpak-org.example.Sheets-3.scope")));
    s.flatpak(pid, "org.example.Sheets");
    let sheets: Option<Caller> = callers.caller_of(&sender).await;
    assert_eq!(sheets.map(|c| c.role), Some(CallerRole::SheetHost));
}

/// The systemd manager, as far as `ProcCallers` asks it: `GetUnit` for the units it was given,
/// each with its `Service.MainPID`.
mod fake_systemd {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use zbus::zvariant::OwnedObjectPath;

    /// Each loaded unit's main process, which the test may change.
    pub type Units = Arc<Mutex<BTreeMap<String, u32>>>;

    pub struct Manager(Units);

    fn path_of(unit: &str) -> OwnedObjectPath {
        let escaped: String = unit
            .bytes()
            .map(|b| match b {
                b if b.is_ascii_alphanumeric() => char::from(b).to_string(),
                b => format!("_{b:02x}"),
            })
            .collect();
        OwnedObjectPath::try_from(format!("/org/freedesktop/systemd1/unit/{escaped}"))
            .expect("path")
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl Manager {
        #[zbus(name = "GetUnit")]
        fn get_unit(&self, unit: &str) -> zbus::fdo::Result<OwnedObjectPath> {
            match self.0.lock().expect("units").contains_key(unit) {
                true => Ok(path_of(unit)),
                false => Err(zbus::fdo::Error::Failed(format!("Unit {unit} not loaded."))),
            }
        }
    }

    pub struct Service {
        unit: String,
        units: Units,
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Service")]
    impl Service {
        #[zbus(property, name = "MainPID")]
        fn main_pid(&self) -> u32 {
            self.units
                .lock()
                .expect("units")
                .get(&self.unit)
                .copied()
                .unwrap_or(0)
        }
    }

    /// Serves the manager for `units` on `connection` under the manager's name.
    pub async fn serve(connection: &zbus::Connection, units: Units) {
        let objects = connection.object_server();
        let loaded: Vec<String> = units.lock().expect("units").keys().cloned().collect();
        for unit in loaded {
            let service = Service {
                unit: unit.clone(),
                units: Arc::clone(&units),
            };
            objects.at(path_of(&unit), service).await.expect("unit");
        }
        objects
            .at("/org/freedesktop/systemd1", Manager(units))
            .await
            .expect("manager");
        connection
            .request_name("org.freedesktop.systemd1")
            .await
            .expect("name");
    }
}

#[tokio::test]
async fn on_a_private_bus_a_service_is_its_unit_only_as_the_managers_main_process() {
    let s = Scratch::new();
    let bus = PrivateBus::start();
    let client = bus.connect().await;
    let sender = client.unique_name().expect("unique name").to_string();
    let service = bus.connect().await;
    let manager = bus.connect().await;
    let pid = std::process::id();
    s.process(
        pid,
        Some("/user.slice/user@1000.service/app.slice/inferd.service"),
    );
    // The fixture says this process is the main process; the manager is asked instead.
    s.main_of("inferd.service", pid);
    let callers = ProcCallers::with_proc_root(service.clone(), table(), s.path().to_owned())
        .with_main_pids(MainPids::Manager);

    // No manager on the bus: no service is anyone.
    assert_eq!(callers.caller_of(&sender).await, None);

    // The manager names another process as the unit's main one: this one only moved in.
    let units: fake_systemd::Units = Default::default();
    units
        .lock()
        .expect("units")
        .insert("inferd.service".to_owned(), pid + 1);
    fake_systemd::serve(&manager, std::sync::Arc::clone(&units)).await;
    assert_eq!(callers.caller_of(&sender).await, None);

    // The manager names this process.
    units
        .lock()
        .expect("units")
        .insert("inferd.service".to_owned(), pid);
    let caller = callers.caller_of(&sender).await.expect("the daemon");
    assert_eq!(
        (caller.app.name.as_str(), caller.role),
        ("org.quire.Inference", CallerRole::PorterDaemon)
    );

    // A unit the manager has not loaded is nobody's.
    s.process(pid, Some("/system.slice/cuad.service"));
    s.main_of("cuad.service", pid);
    assert_eq!(callers.caller_of(&sender).await, None);
}
