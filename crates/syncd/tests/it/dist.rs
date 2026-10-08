//! What ships: the unit and the activation file agree, the unit leaves `/proc` readable (callers
//! are found through it), writes only syncd's own directories, and the test-only proc-root
//! feature is never default.

use std::path::PathBuf;

fn dist(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../dist")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn value<'a>(file: &'a str, key: &str) -> Option<&'a str> {
    file.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
}

#[test]
fn the_unit_and_the_activation_file_name_the_same_service() {
    let unit = dist("syncd.service");
    let activation = dist("dbus/org.quire.Sync1.service");
    assert_eq!(value(&unit, "BusName"), Some("org.quire.Sync1"));
    assert_eq!(value(&unit, "Type"), Some("dbus"));
    assert_eq!(value(&activation, "Name"), Some("org.quire.Sync1"));
    assert_eq!(value(&activation, "SystemdService"), Some("syncd.service"));
    assert_eq!(value(&activation, "Exec"), value(&unit, "ExecStart"));
    assert_eq!(value(&unit, "ExecStart"), Some("/usr/libexec/quire/syncd"));
}

#[test]
fn the_unit_does_not_hide_the_processes_it_must_identify() {
    let unit = dist("syncd.service");
    for hiding in ["ProtectProc", "PrivatePIDs", "ProcSubset"] {
        assert!(
            value(&unit, hiding).is_none(),
            "{hiding} would blind /proc/<pid>/cgroup"
        );
    }
}

#[test]
fn the_unit_is_a_sandbox_that_writes_only_its_own_directories() {
    let unit = dist("syncd.service");
    assert_eq!(value(&unit, "NoNewPrivileges"), Some("yes"));
    assert_eq!(value(&unit, "CapabilityBoundingSet"), Some(""));
    assert_eq!(value(&unit, "ProtectSystem"), Some("strict"));
    assert_eq!(value(&unit, "ProtectHome"), Some("read-only"));
    // Journals under state, mirrors under data: nothing else is writable, and both exist before
    // the sandbox starts (a missing ReadWritePaths entry fails the unit).
    assert_eq!(value(&unit, "StateDirectory"), Some("porter/sync"));
    assert_eq!(
        value(&unit, "ReadWritePaths"),
        Some("%h/.local/share/porter/vdir")
    );
    assert_eq!(
        value(&unit, "ExecStartPre"),
        Some("+/usr/bin/mkdir -p %h/.local/share/porter/vdir")
    );
    // The bus and the replicas' servers (through accountd's relay, syncd itself holds no
    // credential and dials only its own session bus): Unix sockets only.
    assert_eq!(value(&unit, "RestrictAddressFamilies"), Some("AF_UNIX"));
}

#[test]
fn the_test_proc_root_feature_is_not_a_default_feature() {
    let manifest: toml::Table = include_str!("../../Cargo.toml").parse().expect("manifest");
    let features = manifest["features"].as_table().expect("features");
    assert!(features.contains_key("test-proc-root"));
    let default = features
        .get("default")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        default.iter().all(|f| f.as_str() != Some("test-proc-root")),
        "{default:?}"
    );
}

#[test]
fn syncd_is_one_app_id_in_the_caller_table_and_in_every_test_and_doc() {
    let table = dist("callers.toml");
    assert!(
        table.contains(
            "app = \"org.quire.Sync\"\nunit = \"syncd.service\"\nrole = \"porter_daemon\""
        ),
        "the shipped caller table names syncd `org.quire.Sync`"
    );
    // The name this crate once also went by; no source, test or doc may use it again.
    let stray = ["org.quire.", "Syncd"].concat();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut pending = vec![
        root.join("src"),
        root.join("tests"),
        root.join("../../FINDINGS.md"),
    ];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(
                std::fs::read_dir(&path)
                    .expect("dir")
                    .map(|e| e.expect("entry").path()),
            );
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            assert!(!text.contains(&stray), "{} names `{stray}`", path.display());
        }
    }
}

#[test]
fn the_shipped_rescan_is_ten_minutes_and_only_a_test_build_can_change_it() {
    let ten_minutes = std::time::Duration::from_secs(600);
    assert_eq!(
        syncd::datasets::storage::StorageConfig::default().rescan,
        ten_minutes
    );
    assert_eq!(
        syncd::datasets::pim::PimConfig::default().rescan,
        ten_minutes
    );
    let main = include_str!("../../src/main.rs");
    // The variable goes through `paths::rescan`, which ignores it outside a test build.
    assert!(
        main.contains("rescan(BUILD, rescan_var"),
        "main reads it by build"
    );
}
