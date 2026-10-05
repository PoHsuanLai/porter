//! `ProcCallers` over fixture `/proc` trees (the pure resolution) and a
//! private bus where an unidentified sender is refused. Nothing reads the real `/proc` or
//! `/etc`: the roots and the table paths are the test's own.

#[path = "callers/bus.rs"]
mod bus;

use bus::PrivateBus;
use porter_core::Isolation;
use porter_dbus::{Caller, CallerRole, CallerTable, Callers, ProcCallers};
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
fn nothing_but_the_cgroup_file_is_read() {
    use std::os::unix::fs::PermissionsExt;
    let s = Scratch::new();
    s.process(50, Some(&scope("app-flatpak-org.example.Photos-5.scope")));
    s.process(51, Some("/system.slice/inferd.service"));
    // `exe` and `root` are there and unreadable, as ptrace-gated entries are under Landlock.
    for pid in [50, 51] {
        let dir = s.path().join(pid.to_string());
        for entry in ["exe", "root"] {
            std::fs::create_dir(dir.join(entry)).expect("entry");
            std::fs::set_permissions(dir.join(entry), std::fs::Permissions::from_mode(0o0))
                .expect("chmod");
        }
    }
    assert_eq!(
        resolve(&s, 50),
        got("org.example.Photos", Isolation::Flatpak, CallerRole::App)
    );
    assert_eq!(
        resolve(&s, 51),
        got(
            "org.quire.Inference",
            Isolation::Unsandboxed,
            CallerRole::PorterDaemon
        )
    );
    for pid in [50, 51] {
        for entry in ["exe", "root"] {
            let path = s.path().join(pid.to_string()).join(entry);
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
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
    let sheets: Option<Caller> = callers.caller_of(&sender).await;
    assert_eq!(sheets.map(|c| c.role), Some(CallerRole::SheetHost));
}
