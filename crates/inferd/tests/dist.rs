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

fn schema_keys() -> Vec<toml::Table> {
    let schema: toml::Table = dist("inferd.settings.toml")
        .parse()
        .expect("the schema is TOML");
    schema["key"]
        .as_array()
        .expect("key tables")
        .iter()
        .map(|key| key.as_table().expect("a table").clone())
        .collect()
}

#[test]
fn the_schema_rows_are_the_four_structured_rows_with_the_codes_defaults_and_ranges() {
    use inferd::structured::limits::{DEPTH, OPEN_LIST, OPEN_TEXT, REPAIR_BUDGET};
    let keys = schema_keys();
    let rows = [OPEN_TEXT, OPEN_LIST, DEPTH, REPAIR_BUDGET];
    assert_eq!(keys.len(), rows.len());
    for (key, row) in keys.iter().zip(rows) {
        assert_eq!(key["path"].as_str(), Some(row.path));
        assert_eq!(key["default"].as_integer(), Some(i64::from(row.default)));
        let kind = key["kind"]["v"].as_table().expect("bounded");
        assert_eq!(
            kind["min"].as_integer(),
            Some(i64::from(*row.range.start()))
        );
        assert_eq!(kind["max"].as_integer(), Some(i64::from(*row.range.end())));
        assert_eq!(key["page"]["kind"].as_str(), Some("intelligence"));
    }
}

#[test]
fn every_ai_row_is_hands_off() {
    // design/22 section 9.7: `ai.` is in AGENT_NEVER_SETTABLE; a row marked settable would be
    // refused by the schema loader.
    for key in schema_keys() {
        assert!(key["path"].as_str().expect("path").starts_with("ai."));
        assert_eq!(key.get("agent"), None, "{}", key["path"]);
    }
}

#[test]
fn the_schema_defaults_written_as_a_file_resolve_to_the_defaults() {
    let mut structured = toml::Table::new();
    for key in schema_keys() {
        let name = key["path"]
            .as_str()
            .expect("path")
            .rsplit('.')
            .next()
            .expect("name");
        structured.insert(name.to_owned(), key["default"].clone());
    }
    let text = toml::to_string(&toml::Table::from_iter([(
        "ai".to_owned(),
        toml::Value::Table(toml::Table::from_iter([(
            "structured".to_owned(),
            toml::Value::Table(structured),
        )])),
    )]))
    .expect("toml");
    let config = InferdConfig::from_toml(&text).expect("reads");
    let resolved = config.ai.resolve();
    assert_eq!(resolved.limits, inferd::structured::Limits::default());
    assert!(resolved.rejected.is_empty());
}
