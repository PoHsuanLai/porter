use super::*;
use porter_core::Isolation;

fn table() -> CallerTable {
    CallerTable::from_toml_text(
        r#"
cua = ["cuad.service"]
agent_launcher = ["docket-acp.service"]
[apps]
"org.quire.Memory" = ["memoryd.service"]
"org.quire.Mail" = ["mailo.service", "mailo-dev.service"]
"org.quire.Sneaky" = ["cuad.service", "docket-acp.service"]
"#,
    )
    .expect("table")
}

fn named(caller: &Caller) -> (&str, Role, Isolation) {
    (caller.app.name.as_str(), caller.role, caller.app.isolation)
}

#[test]
fn a_unit_is_the_caller_the_table_names_for_it() {
    let table = table();
    let memory = table.resolve("memoryd.service").expect("memoryd");
    assert_eq!(
        named(&memory),
        ("org.quire.Memory", Role::App, Isolation::Unsandboxed)
    );
    for exe in ["mailo.service", "mailo-dev.service"] {
        let mail = table.resolve(exe).expect("mailo");
        assert_eq!(mail.app.name.as_str(), "org.quire.Mail");
    }
}

#[test]
fn cuad_is_the_one_computer_use_caller_and_an_app_entry_cannot_claim_its_unit() {
    let cuad = table().resolve("cuad.service").expect("cuad");
    assert_eq!(
        named(&cuad),
        ("org.quire.Cua", Role::Cua, Isolation::Unsandboxed)
    );
}

#[test]
fn the_agent_launcher_is_the_unit_the_table_names_and_an_app_entry_cannot_claim_it() {
    let launcher = table().resolve("docket-acp.service").expect("the launcher");
    assert_eq!(
        named(&launcher),
        (
            "org.quire.AgentLauncher",
            Role::AgentLauncher,
            Isolation::Unsandboxed
        )
    );
    assert_eq!(
        CallerTable::default()
            .with_agent_launcher(["x.service".to_owned()].into())
            .resolve("x.service")
            .map(|c| c.role),
        Some(Role::AgentLauncher)
    );
}

#[test]
fn a_program_the_table_does_not_name_is_nobody() {
    let table = table();
    for exe in ["bash.service", "memoryd.service.d", ""] {
        assert_eq!(table.resolve(exe), None, "{exe:?}");
    }
    assert_eq!(CallerTable::default().resolve("mailo.service"), None);
}

#[test]
fn a_table_with_a_malformed_app_name_does_not_parse() {
    assert!(CallerTable::from_toml_text("[apps]\n\"not a name\" = [\"x.service\"]\n").is_err());
    assert!(CallerTable::from_toml_text("cua = 3").is_err());
}

#[tokio::test]
async fn introduced_connections_are_known_and_others_are_not() {
    let peers = TablePeers::new();
    let caller = table().resolve("mailo.service").expect("mailo");
    peers.introduce(":1.42", caller.clone());
    assert_eq!(peers.caller_of(":1.42").await, Some(caller.clone()));
    assert_eq!(peers.caller_of(":1.43").await, None);
    let shared = std::sync::Arc::new(peers);
    assert_eq!(shared.caller_of(":1.42").await, Some(caller));
}

#[test]
fn the_table_is_rows_of_the_shared_table_with_cuad_first() {
    let rows = table().rows();
    let roles: Vec<_> = rows.callers.iter().map(|row| row.role).collect();
    assert_eq!(roles[0], porter_dbus::CallerRole::Cua);
    assert_eq!(roles[1], porter_dbus::CallerRole::AgentLauncher);
    assert!(
        roles[2..]
            .iter()
            .all(|r| *r == porter_dbus::CallerRole::App)
    );
    assert_eq!(rows.callers.len(), 7);
    // The first row for cuad's unit is cuad's, not the sneaky app's.
    let cuad = rows.resolve_unit("cuad.service").expect("cuad");
    assert_eq!(cuad.role, porter_dbus::CallerRole::Cua);
    let launcher = rows.resolve_unit("docket-acp.service").expect("launcher");
    assert_eq!(launcher.role, porter_dbus::CallerRole::AgentLauncher);
}

/// sec-3 follow-up: the settings module's caller is the Settings app's unit, not its name. An
/// app-name entry (the old form) stands for `<app>.service`; the row comes before the apps', so
/// an app entry cannot claim the unit; a malformed entry is refused.
#[test]
fn settings_is_a_unit_and_an_app_name_stands_for_its_service() {
    for entry in ["org.quire.Settings.service", "org.quire.Settings"] {
        let table = CallerTable::from_toml_text(&format!(
            "settings = [\"{entry}\"]\n[apps]\n\"org.example.Sneaky\" = [\"org.quire.Settings.service\"]\n"
        ))
        .expect("table");
        let rows = table.rows();
        let settings: Vec<_> = rows
            .callers
            .iter()
            .filter(|row| row.role == porter_dbus::CallerRole::Settings)
            .map(|row| (row.unit.clone(), row.app.to_string()))
            .collect();
        assert_eq!(
            settings,
            [(
                Some("org.quire.Settings.service".to_owned()),
                "org.quire.Settings".to_owned()
            )],
            "{entry}"
        );
        let caller = table
            .resolve("org.quire.Settings.service")
            .expect("the unit");
        assert_eq!(
            named(&caller),
            ("org.quire.Settings", Role::Settings, Isolation::Unsandboxed),
            "{entry}"
        );
    }
    for bad in ["not a name", ".service"] {
        assert!(
            CallerTable::from_toml_text(&format!("settings = [\"{bad}\"]\n")).is_err(),
            "{bad}"
        );
    }
}

/// sec-3 follow-up, through porter_dbus's fixture tree (`<root>/<pid>/cgroup`,
/// `<root>/units/<unit>` for a unit's main process): only the main process of the Settings unit
/// has the role; a process moved into its cgroup, and one in an app scope named like the app,
/// do not.
#[test]
fn only_the_settings_units_main_process_is_settings() {
    let root = std::env::temp_dir().join(format!("inferd-settings-unit-{}", std::process::id()));
    let slice = "/user.slice/user-1000.slice/user@1000.service/app.slice";
    let process = |pid: u32, unit: &str| {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).expect("proc dir");
        std::fs::write(dir.join("cgroup"), format!("0::{slice}/{unit}\n")).expect("cgroup");
    };
    process(20, "org.quire.Settings.service");
    process(21, "org.quire.Settings.service");
    process(22, "app-org.quire.Settings-7.scope");
    std::fs::create_dir_all(root.join("units")).expect("units");
    std::fs::write(root.join("units/org.quire.Settings.service"), "20\n").expect("main pid");
    let rows = CallerTable::from_toml_text("settings = [\"org.quire.Settings.service\"]\n")
        .expect("table")
        .rows();
    let role =
        |pid| porter_dbus::ProcCallers::caller_of_pid(&root, pid, &rows).map(|caller| caller.role);
    assert_eq!(role(20), Some(porter_dbus::CallerRole::Settings));
    assert_eq!(role(21), None, "in the unit's cgroup, not its main process");
    assert_ne!(
        role(22),
        Some(porter_dbus::CallerRole::Settings),
        "an app scope of the app's name"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_proc_root_variable_is_honoured_only_by_a_test_build() {
    use std::path::PathBuf;
    let dir = || PathBuf::from("/fake");
    let cases = [
        (ProcGate::Honour, None, ProcRoot::System),
        (ProcGate::Honour, Some(""), ProcRoot::System),
        (ProcGate::Honour, Some("/fake"), ProcRoot::Fake(dir())),
        (ProcGate::Ignore, None, ProcRoot::System),
        (ProcGate::Ignore, Some("/fake"), ProcRoot::Ignored(dir())),
    ];
    for (gate, var, expected) in cases {
        let got = ProcRoot::select(gate, var);
        assert_eq!(got, expected, "{gate:?} {var:?}");
        assert_eq!(got.notice().is_some(), var.is_some_and(|v| !v.is_empty()));
    }
    assert!(
        ProcRoot::Ignored(dir())
            .notice()
            .expect("line")
            .contains("ignored")
    );
    assert_eq!(
        ProcGate::BUILT == ProcGate::Honour,
        cfg!(feature = "test-proc-root")
    );
}
