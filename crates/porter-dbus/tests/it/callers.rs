//! `ProcCallers` over fixture `/proc` trees (peephole's judgement of a process, then the table's
//! roles) and a private bus where an unidentified sender is refused. Nothing reads the real
//! `/proc` or `/etc`: the roots and the table paths are the test's own. Needs feature
//! `test-proc-root` (main.rs declares this module only then).

#[path = "callers/bus.rs"]
mod bus;

use bus::PrivateBus;
use porter_core::Isolation;
use porter_dbus::{Caller, CallerRole, CallerTable, Callers, ProcCallers};
use porter_fake::Scratch;
use std::path::Path;

const SLICE: &str = "/user.slice/user-1000.slice/user@1000.service/app.slice";

/// A fixture `/proc` tree in a scratch directory.
struct Procs(Scratch);

impl Procs {
    fn new() -> Self {
        Self(Scratch::new("callers"))
    }

    fn path(&self) -> &Path {
        self.0.path()
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

    /// Process `pid` runs the program `image` (a root-owned one, as the fixture world has them).
    fn image(&self, pid: u32, image: &str) {
        let link = self.0.join(pid.to_string()).join("exe");
        std::os::unix::fs::symlink(image, link).expect("exe link");
    }

    /// Process `pid` is the main process of the service `unit`.
    fn main_of(&self, unit: &str, pid: u32) {
        let units = self.0.join("units");
        std::fs::create_dir_all(&units).expect("units");
        std::fs::write(units.join(unit), format!("{pid}\n")).expect("main pid");
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
        {"app": "org.quire.Shell", "unit": "sill-shell.scope", "role": "sheet_host",
         "exe": ["/usr/bin/sill"]},
        {"app": "org.quire.Shell", "unit": "sill.service", "role": "sheet_host",
         "exe": ["/usr/bin/sill"]},
        {"app": "org.example.Odd", "unit": "odd.scope", "role": "settings",
         "exe": ["/usr/bin/odd"]},
        {"app": "org.quire.Trap", "unit": "app-org.quire.Trap-1.scope", "role": "settings"},
    ]}))
    .expect("table")
}

fn scope(unit: &str) -> String {
    format!("{SLICE}/{unit}")
}

fn resolve(scratch: &Procs, pid: u32) -> Option<(String, Isolation, CallerRole)> {
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
    let s = Procs::new();
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
        // Fixed-role services, shown to be their units' main processes: they keep the role.
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
fn a_scope_app_is_unsandboxed_and_a_flatpak_is_flatpak() {
    let s = Procs::new();
    s.process(80, Some(&scope("app-org.example.Mail-1.scope")));
    s.process(81, Some(&scope("app-flatpak-org.example.Photos-2.scope")));
    s.flatpak(81, "org.example.Photos");
    assert_eq!(resolve(&s, 80).map(|c| c.1), Some(Isolation::Unsandboxed));
    assert_eq!(resolve(&s, 81).map(|c| c.1), Some(Isolation::Flatpak));
}

#[test]
fn a_scope_named_after_a_daemons_app_is_an_app() {
    // Any process may start a scope by any name: only the unit itself carries the unit's role.
    let s = Procs::new();
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
fn a_scope_a_row_names_as_its_unit_is_that_unit_exactly_and_no_more_than_an_app() {
    let s = Procs::new();
    s.process(40, Some(&scope("sill-shell.scope")));
    s.image(40, "/usr/bin/sill");
    s.process(41, Some(&scope("sill-shell-2.scope")));
    s.process(42, Some(&scope("xsill-shell.scope")));
    // An app scope never takes a unit row's role, even one naming it exactly.
    s.process(43, Some(&scope("app-org.quire.Trap-1.scope")));
    // A scope has no main process, so the row's fixed role is a plain app's.
    assert_eq!(
        resolve(&s, 40),
        got("org.quire.Shell", Isolation::Unsandboxed, CallerRole::App)
    );
    assert_eq!(resolve(&s, 41), None);
    assert_eq!(resolve(&s, 42), None);
    assert_eq!(
        resolve(&s, 43),
        got("org.quire.Trap", Isolation::Unsandboxed, CallerRole::App)
    );
}

#[test]
fn the_shell_as_a_service_keeps_its_role_only_as_the_units_main_process() {
    let s = Procs::new();
    let unit = "/user.slice/user-1000.slice/user@1000.service/app.slice/sill.service";
    s.process(60, Some(unit));
    s.process(61, Some(unit));
    s.image(60, "/usr/bin/sill");
    s.image(61, "/usr/bin/sill");
    s.main_of("sill.service", 60);
    assert_eq!(
        resolve(&s, 60),
        got(
            "org.quire.Shell",
            Isolation::Unsandboxed,
            CallerRole::SheetHost
        )
    );
    // The genuine program in the unit's cgroup that is not its main process: shown by its
    // program only, so a plain app.
    assert_eq!(
        resolve(&s, 61),
        got("org.quire.Shell", Isolation::Unsandboxed, CallerRole::App)
    );
}

#[test]
fn a_scope_row_not_marked_as_a_transition_gives_no_fixed_role() {
    let s = Procs::new();
    s.process(62, Some(&scope("odd.scope")));
    s.image(62, "/usr/bin/odd");
    assert_eq!(
        resolve(&s, 62),
        got("org.example.Odd", Isolation::Unsandboxed, CallerRole::App)
    );
}

#[test]
fn a_process_nothing_names_is_nobody() {
    let s = Procs::new();
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

/// A `root` closed to the reader (another user's process, or ptrace-gated under Landlock) is no
/// sandbox metadata, as peephole 0.1.3 reads it, not a failed probe: the cgroup and the image
/// decide. A Flatpak scope then disagrees with "native" and is nobody; a service is its unit's
/// main process; an app scope is the app it names, as a claim.
#[test]
fn an_unreadable_sandbox_root_leaves_the_cgroup_and_the_image_to_decide() {
    use std::os::unix::fs::PermissionsExt;
    let s = Procs::new();
    s.process(50, Some(&scope("app-flatpak-org.example.Photos-5.scope")));
    s.process(51, Some("/system.slice/inferd.service"));
    s.process(52, Some(&scope("app-org.example.Mail-4242.scope")));
    s.main_of("inferd.service", 51);
    // `root` is there and unreadable, as ptrace-gated entries are under Landlock.
    for pid in [50, 51, 52] {
        let root = s.path().join(pid.to_string()).join("root");
        std::fs::create_dir(&root).expect("root");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o0)).expect("chmod");
    }
    assert_eq!(resolve(&s, 50), None, "a Flatpak scope with no metadata");
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
        let root = s.path().join(pid.to_string()).join("root");
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

#[test]
fn a_flatpak_app_is_named_by_its_metadata_checked_against_its_scope() {
    let s = Procs::new();
    // A Flatpak scope with no metadata: any program can start a scope by that name.
    s.process(60, Some(&scope("app-flatpak-org.example.Photos-5.scope")));
    // Metadata that disagrees with the cgroup (another app's scope, a daemon's unit): nobody.
    s.process(61, Some(&scope("app-org.quire.Settings-1.scope")));
    s.flatpak(61, "org.example.Photos");
    s.process(62, Some("/system.slice/inferd.service"));
    s.flatpak(62, "org.example.Photos");
    s.main_of("inferd.service", 62);
    // Metadata that names no application: nobody, in any scope.
    s.process(63, Some(&scope("app-org.example.Mail-1.scope")));
    let root = s.path().join("63/root");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join(".flatpak-info"), "[Instance]\ninstance-id=1\n").expect("info");
    for pid in [60, 61, 62, 63] {
        assert_eq!(resolve(&s, pid), None, "pid {pid}");
    }
}

#[test]
fn a_service_role_goes_only_to_the_units_main_process() {
    let s = Procs::new();
    let unit = "/user.slice/user@1000.service/app.slice/inferd.service";
    s.process(70, Some(unit));
    s.main_of("inferd.service", 70);
    // Another process of the person's that moved itself into the unit's cgroup: it runs no
    // program of the row.
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

#[test]
fn a_service_whose_program_is_not_the_rows_is_nobody() {
    let s = Procs::new();
    s.process(
        90,
        Some("/user.slice/user@1000.service/app.slice/inferd.service"),
    );
    s.image(90, "/usr/bin/other");
    s.main_of("inferd.service", 90);
    assert_eq!(resolve(&s, 90), None);
}

#[tokio::test]
async fn on_a_private_bus_an_unidentified_sender_is_refused_and_an_identified_one_is_named() {
    let s = Procs::new();
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

    // And as a listed Flatpak app.
    s.process(pid, Some(&scope("app-flatpak-org.example.Sheets-3.scope")));
    s.flatpak(pid, "org.example.Sheets");
    let sheets: Option<Caller> = callers.caller_of(&sender).await;
    assert_eq!(sheets.map(|c| c.role), Some(CallerRole::SheetHost));
}

#[tokio::test]
async fn on_a_private_bus_a_service_is_its_unit_only_as_the_units_main_process() {
    let s = Procs::new();
    let bus = PrivateBus::start();
    let client = bus.connect().await;
    let sender = client.unique_name().expect("unique name").to_string();
    let service = bus.connect().await;
    let pid = std::process::id();
    s.process(
        pid,
        Some("/user.slice/user@1000.service/app.slice/inferd.service"),
    );
    let callers = ProcCallers::with_proc_root(service.clone(), table(), s.path().to_owned());

    // The tree states no main process: this one only moved in.
    assert_eq!(callers.caller_of(&sender).await, None);
    s.main_of("inferd.service", pid + 1);
    assert_eq!(callers.caller_of(&sender).await, None);

    // The tree names this process.
    s.main_of("inferd.service", pid);
    let caller = callers.caller_of(&sender).await.expect("the daemon");
    assert_eq!(
        (caller.app.name.as_str(), caller.role),
        ("org.quire.Inference", CallerRole::PorterDaemon)
    );

    // A unit the tree has not loaded is nobody's.
    s.process(pid, Some("/system.slice/cuad.service"));
    assert_eq!(callers.caller_of(&sender).await, None);
}
