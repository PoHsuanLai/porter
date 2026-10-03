//! The files under `dist/` agree with the code: the activation file and the unit name the bus name
//! the daemon claims, and the sample configuration reads.

use inferd::config::InferdConfig;
use std::path::{Path, PathBuf};

fn dist(file: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../dist")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn key<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{name}=")))
}

#[test]
fn the_activation_file_and_the_unit_name_the_bus_name_and_each_other() {
    let activation = dist("dbus/org.quire.Inference1.service");
    let unit = dist("inferd.service");
    assert_eq!(key(&activation, "Name"), Some(porter_dbus::INFERENCE_BUS));
    assert_eq!(key(&unit, "BusName"), Some(porter_dbus::INFERENCE_BUS));
    assert_eq!(key(&activation, "SystemdService"), Some("inferd.service"));
    assert_eq!(key(&activation, "Exec"), key(&unit, "ExecStart"));
    assert_eq!(key(&unit, "Type"), Some("dbus"));
}

#[test]
fn the_unit_gives_engines_the_gpu_and_the_network_to_nobody() {
    let unit = dist("inferd.service");
    assert_eq!(key(&unit, "PrivateNetwork"), Some("yes"));
    assert_eq!(key(&unit, "RestrictAddressFamilies"), Some("AF_UNIX"));
    assert_eq!(key(&unit, "DevicePolicy"), Some("closed"));
    // Engines' JIT needs writable and executable memory, so the one line cuad's unit has is absent.
    assert_eq!(key(&unit, "MemoryDenyWriteExecute"), None);
    assert!(unit.lines().any(|line| line.starts_with("DeviceAllow=")));
}

#[test]
fn the_sample_configuration_reads_and_names_the_callers_the_design_calls_for() {
    let config = InferdConfig::from_toml(&dist("inferd.toml")).expect("the sample reads");
    let cua = config
        .callers
        .resolve(Path::new("/usr/libexec/quire/cuad"))
        .expect("cuad");
    assert_eq!(cua.role, inferd::peers::Role::Cua);
    for exe in ["memoryd", "intentd", "companiond", "readerd"] {
        let path = PathBuf::from("/usr/libexec/quire").join(exe);
        let caller = config
            .callers
            .resolve(&path)
            .unwrap_or_else(|| panic!("{exe}"));
        assert_eq!(caller.role, inferd::peers::Role::App);
    }
}
