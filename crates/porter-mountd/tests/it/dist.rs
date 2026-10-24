//! The helper's units as shipped: the socket syncd finds, and a service whose every setting keeps
//! NoNewPrivileges off and makes no mount namespace, so fusermount3 keeps working and the mount
//! is seen by the session.

use porter_mountd::wire;
use std::path::{Path, PathBuf};

fn dist(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../dist")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The `key=value` lines of `section`, in order.
fn section<'a>(file: &'a str, name: &str) -> Vec<(&'a str, &'a str)> {
    let mut inside = false;
    let mut found = Vec::new();
    for line in file.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == format!("[{name}]");
        } else if inside && !line.is_empty() && !line.starts_with('#') {
            found.push(line.split_once('=').expect("key=value"));
        }
    }
    found
}

#[test]
fn the_service_has_only_settings_that_keep_fusermount3_working() {
    let unit = dist("porter-mountd.service");
    let keys: Vec<&str> = section(&unit, "Service").iter().map(|(k, _)| *k).collect();
    assert_eq!(
        keys,
        [
            "UnsetEnvironment",
            "Type",
            "ExecStart",
            "Restart",
            "RestartSec",
            "IPAddressDeny",
            "KeyringMode",
            "TasksMax",
            "MemoryMax",
            "UMask",
        ],
        "a setting added here must be checked with `systemd-run --user -p <setting> grep \
         NoNewPrivs /proc/self/status` (0) and make no mount or user namespace"
    );
    // Each of these turns NoNewPrivileges on in a user manager (its seccomp filter needs it), or
    // is it, or makes a user namespace (where fusermount3 is not root) or a mount namespace
    // (which would hide the mount): none may come back.
    for breaks in [
        "NoNewPrivileges",
        "RestrictSUIDSGID",
        "CapabilityBoundingSet",
        "AmbientCapabilities",
        "LockPersonality",
        "MemoryDenyWriteExecute",
        "RestrictRealtime",
        "RestrictNamespaces",
        "RestrictAddressFamilies",
        "SystemCallArchitectures",
        "SystemCallFilter",
        "ProtectClock",
        "ProtectHostname",
        "ProtectKernelTunables",
        "ProtectKernelModules",
        "ProtectKernelLogs",
        "ProtectHome",
        "ProtectSystem",
        "PrivateTmp",
        "PrivateDevices",
        "PrivateMounts",
        "ProtectControlGroups",
        "ProtectProc",
        "PrivatePIDs",
        "ReadWritePaths",
        "ReadOnlyPaths",
        "InaccessiblePaths",
    ] {
        assert!(!keys.contains(&breaks), "{breaks} would break mounting");
        // And each is named in the header with its reason.
        assert!(
            unit.contains(breaks),
            "the header says why {breaks} is left out"
        );
    }
    let value = |key: &str| {
        section(&unit, "Service")
            .into_iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    };
    assert_eq!(value("ExecStart"), Some("/usr/libexec/quire/porter-mountd"));
    assert_eq!(value("IPAddressDeny"), Some("any"));
    assert_eq!(value("KeyringMode"), Some("private"));
    assert_eq!(value("UMask"), Some("0077"));
    assert_eq!(
        section(&unit, "Unit")
            .into_iter()
            .find(|(k, _)| *k == "Requires"),
        Some(("Requires", "porter-mountd.socket"))
    );
}

#[test]
fn the_socket_is_where_syncd_looks_for_it_and_only_the_person_reaches_it() {
    let socket = dist("porter-mountd.socket");
    let keys = section(&socket, "Socket");
    // %t is the runtime directory, as `wire::socket_in` takes it.
    assert_eq!(
        wire::socket_in(Path::new("%t")),
        Path::new("%t/porter/mountd.sock")
    );
    assert!(keys.contains(&("ListenStream", "%t/porter/mountd.sock")));
    assert!(keys.contains(&("SocketMode", "0600")));
    assert!(keys.contains(&("DirectoryMode", "0700")));
    assert_eq!(
        section(&socket, "Install"),
        [("WantedBy", "sockets.target")]
    );
}
