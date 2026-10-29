//! What ships: the unit and the activation file agree, the unit leaves `/proc` readable (callers
//! are found through it), and the test-only proc-root feature is never default.

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
    let unit = dist("accountd.service");
    let activation = dist("dbus/org.quire.Accounts1.service");
    assert_eq!(value(&unit, "BusName"), Some("org.quire.Accounts1"));
    assert_eq!(value(&unit, "Type"), Some("dbus"));
    assert_eq!(value(&activation, "Name"), Some("org.quire.Accounts1"));
    assert_eq!(
        value(&activation, "SystemdService"),
        Some("accountd.service")
    );
    assert_eq!(value(&activation, "Exec"), value(&unit, "ExecStart"));
    assert_eq!(
        value(&unit, "ExecStart"),
        Some("/usr/libexec/quire/accountd")
    );
}

#[test]
fn the_unit_does_not_restart_over_a_registry_the_daemon_refused() {
    let unit = dist("accountd.service");
    assert_eq!(value(&unit, "Restart"), Some("on-failure"));
    assert_eq!(
        value(&unit, "RestartPreventExitStatus"),
        Some(accountd::EXIT_REGISTRY_REFUSED.to_string().as_str())
    );
}

#[test]
fn the_unit_does_not_hide_the_processes_it_must_identify() {
    let unit = dist("accountd.service");
    for hiding in ["ProtectProc", "PrivatePIDs", "ProcSubset"] {
        assert!(
            value(&unit, hiding).is_none(),
            "{hiding} would blind /proc/<pid>/cgroup"
        );
    }
}

#[test]
fn the_unit_is_a_sandbox_with_the_network_it_needs_and_no_more() {
    let unit = dist("accountd.service");
    assert_eq!(value(&unit, "NoNewPrivileges"), Some("yes"));
    assert_eq!(value(&unit, "CapabilityBoundingSet"), Some(""));
    assert_eq!(
        value(&unit, "RestrictAddressFamilies"),
        Some("AF_UNIX AF_INET AF_INET6")
    );
    // The mount-namespace half of the sandbox is the drop-in, which the installer leaves out
    // where the system refuses a user service its own mount namespace.
    let sandbox = dist("accountd.service.d/sandbox.conf");
    assert_eq!(value(&sandbox, "ProtectSystem"), Some("strict"));
    assert_eq!(value(&sandbox, "PrivateTmp"), Some("yes"));
    for moved in [
        "ProtectSystem",
        "ProtectHome",
        "PrivateTmp",
        "PrivateDevices",
    ] {
        assert_eq!(value(&unit, moved), None, "{moved} belongs in the drop-in");
    }
}

/// accountd holds the AI keys as well now, and its sandbox is exactly what it was: it writes the
/// two state directories and the config directory it already had, nothing for a key (a key travels
/// on a sealed memfd), and it reads inferd's old store through the person's home, which is
/// readable. None of the limits is loosened: the same directives, the same values.
#[test]
fn holding_the_ai_keys_loosens_no_limit_and_adds_no_path() {
    let unit = dist("accountd.service");
    assert_eq!(value(&unit, "RuntimeDirectory"), None, "no place for a key");
    assert_eq!(value(&unit, "ReadWritePaths"), None);
    assert_eq!(
        value(&unit, "StateDirectory"),
        Some("porter quire/accountd")
    );
    assert_eq!(value(&unit, "ConfigurationDirectory"), Some("porter"));
    for (limit, want) in [
        ("MemoryDenyWriteExecute", "yes"),
        ("RestrictNamespaces", "yes"),
        ("RestrictRealtime", "yes"),
        ("RestrictSUIDSGID", "yes"),
        ("LockPersonality", "yes"),
        ("NoNewPrivileges", "yes"),
        ("CapabilityBoundingSet", ""),
        ("AmbientCapabilities", ""),
        ("UMask", "0077"),
    ] {
        assert_eq!(value(&unit, limit), Some(want), "{limit}");
    }
    assert_eq!(
        value(&unit, "IPAddressDeny"),
        None,
        "providers need the network, as before"
    );
    let sandbox = dist("accountd.service.d/sandbox.conf");
    assert_eq!(value(&sandbox, "ProtectHome"), Some("read-only"));
    assert_eq!(value(&sandbox, "ReadWritePaths"), None);
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
fn the_sample_caller_table_gives_settings_the_sheet_host_and_every_daemon_its_role() {
    use porter_callers::CallerRole;
    use porter_core::AppName;
    let table = porter_callers::table_from_toml(&dist("callers.toml")).expect("the sample reads");
    let app = |name: &str| AppName::parse(name).expect("name");
    // Settings is its unit (sec-3 a): an app scope named `org.quire.Settings` is only an app.
    assert_eq!(table.role_of(&app("org.quire.Settings")), CallerRole::App);
    for (unit, role) in [
        ("org.quire.Settings.service", CallerRole::Settings),
        ("sill.service", CallerRole::SheetHost),
        ("inferd.service", CallerRole::AiBroker),
        ("syncd.service", CallerRole::PorterDaemon),
        ("calendard.service", CallerRole::PorterDaemon),
    ] {
        assert_eq!(
            table.resolve_unit(unit).map(|caller| caller.role),
            Some(role),
            "{unit}"
        );
    }
    // Every caller a person may see in an account's Apps group has a name to read.
    for (id, title) in [
        ("org.quire.Settings", "Settings"),
        ("org.quire.Shell", "Shell"),
        ("org.quire.Inference", "Intelligence"),
        ("org.quire.Sync", "Sync"),
        ("org.quire.Calendar", "Calendar"),
        ("org.quire.Companion", "Assistant"),
    ] {
        assert_eq!(table.title_of(&app(id)).map(|t| t.0.as_str()), Some(title));
    }
    // A unit row grants nothing to an app scope named after its app.
    assert_eq!(table.role_of(&app("org.quire.Inference")), CallerRole::App);
}

#[test]
fn what_the_unit_writes_is_created_before_the_sandbox_starts() {
    let unit = dist("accountd.service");
    // A missing ReadWritePaths entry fails the unit; systemd's own directories cannot be missing.
    assert_eq!(
        value(&unit, "StateDirectory"),
        Some("porter quire/accountd")
    );
    assert_eq!(value(&unit, "ConfigurationDirectory"), Some("porter"));
    assert_eq!(value(&unit, "ReadWritePaths"), None);
    let sandbox = dist("accountd.service.d/sandbox.conf");
    assert_eq!(value(&sandbox, "ReadWritePaths"), None);
    assert_eq!(value(&sandbox, "ProtectHome"), Some("read-only"));
}

/// What `[features] default` lists in a crate's manifest, and whether `feature` is declared.
fn default_features(manifest: &str, feature: &str) -> (bool, Vec<String>) {
    let manifest: toml::Table = manifest.parse().expect("manifest");
    let features = manifest["features"].as_table().expect("features");
    let default = features
        .get("default")
        .and_then(|d| d.as_array())
        .map(|d| {
            d.iter()
                .filter_map(|f| f.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    (features.contains_key(feature), default)
}

#[test]
fn the_test_keys_feature_is_declared_and_is_not_a_default_feature_of_accountd_or_porter_secrets() {
    for (manifest, feature) in [
        (include_str!("../../Cargo.toml"), "test-keys"),
        (
            include_str!("../../../porter-secrets/Cargo.toml"),
            "test-keys",
        ),
    ] {
        let (declared, default) = default_features(manifest, feature);
        assert!(declared, "{feature} is declared");
        assert!(
            default.iter().all(|f| !f.contains("test-keys")),
            "default features: {default:?}"
        );
    }
    // And accountd's own feature is the only thing that turns porter-secrets' on.
    let manifest: toml::Table = include_str!("../../Cargo.toml").parse().expect("manifest");
    let dependency = &manifest["dependencies"]["porter-secrets"];
    assert!(
        !dependency.to_string().contains("test-keys"),
        "the normal dependency on porter-secrets does not name test-keys: {dependency}"
    );
}

#[test]
fn nothing_that_ships_names_the_file_key_store() {
    for name in [
        "accountd.service",
        "callers.toml",
        "dbus/org.quire.Accounts1.service",
    ] {
        let file = dist(name);
        for knob in ["ACCOUNTD_KEYS", "test-keys"] {
            assert!(!file.contains(knob), "{name} names {knob}");
        }
    }
}

#[test]
fn no_shipped_row_grants_the_agent_launcher_role_and_the_sample_shows_how_a_machine_lists_it() {
    use porter_callers::CallerRole;
    let shipped = dist("callers.toml");
    let table = porter_callers::table_from_toml(&shipped).expect("the sample reads");
    assert!(
        table
            .callers
            .iter()
            .all(|row| row.role != CallerRole::AgentLauncher),
        "the launcher is listed by the machine that runs it, never by default"
    );
    // The launcher's interface is accountd's now, so its row is documented here, commented out.
    assert!(
        shipped.contains("#   role = \"agent_launcher\""),
        "accountd's sample shows the launcher row"
    );
}
