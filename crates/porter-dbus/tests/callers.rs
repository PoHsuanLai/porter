//! `ProcCallers` over fixture `/proc` trees (the pure resolution), the table files, and a
//! private bus where an unidentified sender is refused. Nothing reads the real `/proc` or
//! `/etc`: the roots and the table paths are the test's own.

#[path = "callers/bus.rs"]
mod bus;

use bus::PrivateBus;
use porter_core::{AppName, Isolation};
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

    /// Process `pid` with this cgroup line, executable and Flatpak info.
    fn process(&self, pid: u32, cgroup: Option<&str>, exe: Option<&str>, info: Option<&str>) {
        let dir = self.0.join(pid.to_string());
        std::fs::create_dir_all(dir.join("root")).expect("proc dir");
        if let Some(cgroup) = cgroup {
            std::fs::write(dir.join("cgroup"), format!("0::{cgroup}\n")).expect("cgroup");
        }
        let _ = std::fs::remove_file(dir.join("exe"));
        if let Some(exe) = exe {
            std::os::unix::fs::symlink(exe, dir.join("exe")).expect("exe");
        }
        if let Some(info) = info {
            std::fs::write(dir.join("root/.flatpak-info"), info).expect("info");
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn table() -> CallerTable {
    CallerTable::from_toml_text(
        r#"
[[caller]]
exe = "/usr/bin/detent"
app = "org.quire.Settings"
role = "settings"

[[caller]]
exe = "/usr/libexec/quire/intentd"
app = "org.quire.Intent"
role = "agent"

[[caller]]
exe = "/usr/libexec/quire/cuad"
app = "org.quire.Cua"
role = "cua"

[[caller]]
exe = "/usr/bin/sill"
app = "org.quire.Sill"
role = "sheet_host"

[[caller]]
exe = "/usr/libexec/quire/inferd"
app = "org.quire.Inference"
role = "porter_daemon"

[[caller]]
exe = "/usr/bin/sheet-host"
app = "org.example.Sheets"
role = "sheet_host"
"#,
    )
    .expect("table")
}

fn flatpak(app: &str) -> String {
    format!("[Application]\nname={app}\n\n[Instance]\ninstance-id=99\n")
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
fn a_process_is_named_by_its_app_scope_flatpak_info_or_executable() {
    let s = Scratch::new();
    s.process(
        10,
        Some(&scope("app-org.example.Mail-4242.scope")),
        Some("/usr/bin/mail"),
        None,
    );
    s.process(
        11,
        Some(&scope("app-gnome-org.example.Calc-77.scope")),
        None,
        None,
    );
    s.process(
        12,
        Some(&scope("app-flatpak-org.example.Photos-5.scope")),
        None,
        Some(&flatpak("org.example.Photos")),
    );
    s.process(
        13,
        Some("/user.slice/session-2.scope"),
        None,
        Some(&flatpak("org.example.Sheets")),
    );
    s.process(
        14,
        Some("/user.slice/session-2.scope"),
        Some("/usr/bin/detent"),
        None,
    );
    s.process(
        15,
        Some("/user.slice/user@1000.service/app.slice/app-org.quire.Mail\\x2dbeta-9.scope"),
        None,
        None,
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
        // A sandboxed app takes the role of its table row.
        (
            13,
            got(
                "org.example.Sheets",
                Isolation::Flatpak,
                CallerRole::SheetHost,
            ),
        ),
        // A native executable in the table, started from anywhere.
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
                "org.quire.Mail-beta",
                Isolation::Unsandboxed,
                CallerRole::App,
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
    // No scope and no Flatpak info; a terminal's child; a launcher's scope that is not an id;
    // a replaced binary; a daemon's service unit with an unlisted executable; a missing pid;
    // a Flatpak info with no app name; no cgroup at all.
    s.process(
        20,
        Some("/user.slice/user-1000.slice/session-2.scope"),
        Some("/usr/bin/bash"),
        None,
    );
    s.process(
        21,
        Some(&scope("vte-spawn-1f2e.scope")),
        Some("/usr/bin/bash"),
        None,
    );
    s.process(
        22,
        Some(&scope("app-firefox-3.scope")),
        Some("/usr/bin/firefox"),
        None,
    );
    s.process(
        23,
        Some(&scope("app-org.example.X-1.scope")),
        Some("/usr/bin/detent (deleted)"),
        None,
    );
    s.process(
        24,
        Some("/system.slice/other.service"),
        Some("/usr/bin/other"),
        None,
    );
    s.process(
        26,
        Some(&scope("app-org.example.Y-1.scope")),
        None,
        Some("[Instance]\ninstance-id=1\n"),
    );
    s.process(27, None, None, None);
    for pid in [20, 21, 22, 24, 25, 26, 27] {
        assert_eq!(resolve(&s, pid), None, "pid {pid}");
    }
    // A replaced binary is not the program that was allowed, but its scope still names an app.
    assert_eq!(
        resolve(&s, 23),
        got("org.example.X", Isolation::Unsandboxed, CallerRole::App)
    );
}

#[test]
fn a_terminal_launched_program_is_known_only_by_its_executable() {
    let s = Scratch::new();
    let terminal = scope("vte-spawn-aa11.scope");
    s.process(30, Some(&terminal), Some("/usr/bin/detent"), None);
    s.process(31, Some(&terminal), Some("/usr/libexec/quire/cuad"), None);
    s.process(
        32,
        Some(&terminal),
        Some("/usr/libexec/quire/intentd"),
        None,
    );
    s.process(33, Some(&terminal), Some("/usr/bin/sill"), None);
    s.process(34, Some(&terminal), Some("/usr/bin/vim"), None);
    let roles: Vec<_> = (30..=34).map(|pid| resolve(&s, pid).map(|c| c.2)).collect();
    assert_eq!(
        roles,
        [
            Some(CallerRole::Settings),
            Some(CallerRole::Cua),
            Some(CallerRole::Agent),
            Some(CallerRole::SheetHost),
            None
        ]
    );
}

#[test]
fn an_executable_row_beats_the_scope_a_program_runs_in() {
    let s = Scratch::new();
    s.process(
        40,
        Some(&scope("app-org.example.Other-1.scope")),
        Some("/usr/libexec/quire/cuad"),
        None,
    );
    assert_eq!(
        resolve(&s, 40),
        got("org.quire.Cua", Isolation::Unsandboxed, CallerRole::Cua)
    );
}

#[test]
fn the_user_file_wins_over_the_system_file() {
    let s = Scratch::new();
    let system = s.path().join("system.toml");
    let user = s.path().join("user.toml");
    std::fs::write(
        &system,
        "[[caller]]\nexe = \"/usr/bin/detent\"\napp = \"org.quire.Settings\"\nrole = \"settings\"\n\
         [[caller]]\nexe = \"/usr/bin/sheet-host\"\napp = \"org.example.Sheets\"\nrole = \"sheet_host\"\n",
    )
    .expect("system");
    std::fs::write(
        &user,
        "[[caller]]\nexe = \"/usr/bin/detent\"\napp = \"org.example.MySettings\"\nrole = \"app\"\n\
         [[caller]]\nexe = \"/opt/sheets\"\napp = \"org.example.Sheets\"\nrole = \"app\"\n",
    )
    .expect("user");
    let merged = CallerTable::load(&system, &user).expect("load");
    let name = |n: &str| AppName::parse(n).expect("name");
    // The same executable: the user's row replaces the system's.
    let detent = merged
        .resolve(Path::new("/usr/bin/detent"))
        .expect("detent");
    assert_eq!(
        (detent.app.name.as_str(), detent.role),
        ("org.example.MySettings", CallerRole::App)
    );
    // The same app under another executable: the user's role is the one that counts.
    assert_eq!(merged.role_of(&name("org.example.Sheets")), CallerRole::App);
    assert_eq!(
        merged.role_of(&name("org.example.Unlisted")),
        CallerRole::App
    );
    // The system file alone still gives the system's role.
    let alone = CallerTable::load(&system, &s.path().join("missing.toml")).expect("alone");
    assert_eq!(
        alone.role_of(&name("org.example.Sheets")),
        CallerRole::SheetHost
    );
}

#[test]
fn a_missing_table_file_is_empty_and_a_broken_one_is_an_error() {
    let s = Scratch::new();
    assert_eq!(
        CallerTable::from_file(&s.path().join("nope.toml")),
        Ok(CallerTable::default())
    );
    let bad = s.path().join("bad.toml");
    std::fs::write(
        &bad,
        "[[caller]]\nexe = \"/x\"\napp = \"org.x.A\"\nrole = \"root\"\n",
    )
    .expect("bad");
    let err = CallerTable::from_file(&bad).expect_err("unknown role");
    assert_eq!(err.path, bad);
    assert!(CallerTable::load(&bad, &s.path().join("nope.toml")).is_err());
}

#[tokio::test]
async fn on_a_private_bus_an_unidentified_sender_is_refused_and_an_identified_one_is_named() {
    let s = Scratch::new();
    let bus = PrivateBus::start();
    let client = bus.connect().await;
    let sender = client.unique_name().expect("unique name").to_string();
    let service = bus.connect().await;
    let pid = std::process::id();

    // The test process has no scope and no Flatpak info in this tree: nobody.
    s.process(
        pid,
        Some("/user.slice/session-2.scope"),
        Some("/usr/bin/bash"),
        None,
    );
    let callers = ProcCallers::with_proc_root(service.clone(), table(), s.path().to_owned());
    assert_eq!(callers.caller_of(&sender).await, None);
    // A name that is not on the bus, and text that is not a name: nobody.
    assert_eq!(callers.caller_of(":1.9999").await, None);
    assert_eq!(callers.caller_of("not a name").await, None);

    // The same process, now in an app's scope.
    s.process(
        pid,
        Some(&scope("app-org.example.Mail-4242.scope")),
        Some("/usr/bin/bash"),
        None,
    );
    let caller = callers.caller_of(&sender).await.expect("identified");
    assert_eq!(
        (caller.app.name.as_str(), caller.app.isolation, caller.role),
        ("org.example.Mail", Isolation::Unsandboxed, CallerRole::App)
    );

    // And as the table's executable.
    s.process(
        pid,
        Some("/user.slice/session-2.scope"),
        Some("/usr/bin/detent"),
        None,
    );
    let settings: Option<Caller> = callers.caller_of(&sender).await;
    assert_eq!(settings.map(|c| c.role), Some(CallerRole::Settings));
}
