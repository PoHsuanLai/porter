//! The files under `dist/` agree with the code: the activation file and the unit name the bus name
//! the daemon claims, and the sample configuration reads.

use ds_settings::schema::{AgentSetting, Exposure, ForeignTables, KeyKind, KeySpec, Page, Schema};
use inferd::config::InferdConfig;
use inferd::settings::{self, CLASSES, slug_of};
use std::path::PathBuf;

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

/// sec-3 follow-up: the shipped table gives the settings module to the Settings app's unit
/// (`org.quire.Settings.service`, as accountd's callers.toml), never by the app's name alone.
#[test]
fn the_shipped_table_names_settings_by_its_unit() {
    let config = InferdConfig::from_toml(&dist("inferd.toml")).expect("the sample reads");
    let rows = config.callers.rows();
    let settings: Vec<_> = rows
        .callers
        .iter()
        .filter(|row| row.role == porter_dbus::CallerRole::Settings)
        .map(|row| (row.unit.as_deref(), row.app.to_string()))
        .collect();
    assert_eq!(
        settings,
        [(
            Some("org.quire.Settings.service"),
            "org.quire.Settings".to_owned()
        )]
    );
}

/// The shipped table lets the companion, reader and intents daemons choose where the assistant
/// runs (by unit, as placers), gives the shell its scope, and leaves the other apps plain.
#[test]
fn the_shipped_table_names_the_placers_by_unit_and_the_shell_by_its_scope() {
    use inferd::peers::Role;
    let config = InferdConfig::from_toml(&dist("inferd.toml")).expect("the sample reads");
    let role = |unit: &str| {
        config
            .callers
            .resolve(unit)
            .map(|c| (c.app.name.to_string(), c.role))
    };
    for (unit, app) in [
        ("companiond.service", "org.quire.Companion"),
        ("readerd.service", "org.quire.Reader"),
        ("intentd.service", "org.quire.Intents"),
    ] {
        assert_eq!(role(unit), Some((app.to_owned(), Role::Placer)), "{unit}");
    }
    assert_eq!(
        role("sill-shell.scope"),
        Some(("org.quire.Shell".to_owned(), Role::Shell))
    );
    assert_eq!(
        role("memoryd.service"),
        Some(("org.quire.Memory".to_owned(), Role::App))
    );
}

#[test]
fn the_unit_gives_engines_the_gpu_and_only_this_computers_own_addresses() {
    let unit = dist("inferd.service");
    // Loopback is reachable (the local runtimes the probe asks, the agent endpoint's listener):
    // no private network, the internet families open, and every address but loopback denied.
    assert_eq!(key(&unit, "PrivateNetwork"), None);
    assert_eq!(
        key(&unit, "RestrictAddressFamilies"),
        Some("AF_UNIX AF_INET AF_INET6")
    );
    assert_eq!(key(&unit, "IPAddressDeny"), Some("any"));
    assert_eq!(key(&unit, "IPAddressAllow"), Some("localhost"));
    assert_eq!(key(&unit, "DevicePolicy"), Some("closed"));
    // Engines' JIT needs writable and executable memory, so the one line cuad's unit has is absent.
    assert_eq!(key(&unit, "MemoryDenyWriteExecute"), None);
    assert!(unit.lines().any(|line| line.starts_with("DeviceAllow=")));
    // What inferd writes is created by systemd before the sandbox starts.
    assert_eq!(key(&unit, "RuntimeDirectory"), Some("inferd"));
    assert_eq!(key(&unit, "StateDirectory"), Some("quire/inferd"));
    assert_eq!(key(&unit, "ReadWritePaths"), None);
}

#[test]
fn the_cloud_drop_in_opens_every_address_and_nothing_else() {
    let drop_in = dist("inferd-cloud.conf");
    assert_eq!(key(&drop_in, "IPAddressAllow"), Some("any"));
    // Only that line: the families and the rest of the sandbox stay as the unit has them.
    let settings: Vec<_> = drop_in
        .lines()
        .filter(|line| !line.starts_with('#') && line.contains('='))
        .collect();
    assert_eq!(settings.len(), 1, "{settings:?}");
}

#[test]
fn the_sample_configuration_reads_and_names_the_callers_the_design_calls_for() {
    let config = InferdConfig::from_toml(&dist("inferd.toml")).expect("the sample reads");
    let cua = config.callers.resolve("cuad.service").expect("cuad");
    assert_eq!(cua.role, inferd::peers::Role::Cua);
    for exe in ["memoryd", "intentd", "companiond", "readerd", "voiced"] {
        let caller = config
            .callers
            .resolve(&format!("{exe}.service"))
            .unwrap_or_else(|| panic!("{exe}"));
        assert_eq!(caller.role, inferd::peers::Role::App);
    }
}

#[test]
fn the_agent_launcher_is_listed_by_the_machine_that_runs_it_never_by_default() {
    let shipped = dist("inferd.toml");
    let config = InferdConfig::from_toml(&shipped).expect("the sample reads");
    assert!(
        config
            .callers
            .rows()
            .callers
            .iter()
            .all(|row| row.role != porter_dbus::CallerRole::AgentLauncher),
        "no launcher by default"
    );
    // The line the sample's comment shows is the one a machine writes, uncommented.
    let documented = shipped
        .lines()
        .find_map(|line| line.strip_prefix("# agent_launcher = "))
        .map(|rest| format!("[callers]\nagent_launcher = {rest}"))
        .expect("the documented line");
    let mine = InferdConfig::from_toml(&documented).expect("the documented line reads");
    let launcher = mine
        .callers
        .resolve("docket-acp.service")
        .expect("the unit");
    assert_eq!(launcher.role, inferd::peers::Role::AgentLauncher);
}

fn schema() -> Schema {
    Schema::from_toml(&dist("inferd.settings.toml")).expect("the schema parses and is hands-off")
}

#[test]
fn the_schema_names_the_three_tables_inferd_keeps_beside_its_rows() {
    let expected = ForeignTables(vec![
        "engines".to_owned(),
        "probe".to_owned(),
        "callers".to_owned(),
    ]);
    assert_eq!(schema().foreign, expected);
}

fn schema_keys() -> Vec<KeySpec> {
    schema().key
}

/// What a configuration makes inferd do, as far as these rows go: the policy as it answers for
/// every class, the tier map, Automatic, the spend line and the structured limits.
fn effect(text: &str) -> (String, Vec<String>) {
    let config = InferdConfig::from_toml(text).unwrap_or_else(|e| panic!("{text}: {e}"));
    let resolved = settings::resolve(&config);
    let s = resolved.settings;
    let floors: Vec<_> = CLASSES.iter().map(|class| s.policy.floor(*class)).collect();
    let limits = config.ai.resolve().limits;
    let mut rejected = resolved.rejected;
    rejected.extend(config.ai.resolve().rejected.iter().map(|p| (*p).to_owned()));
    (
        format!(
            "{:?} {floors:?} {:?} {:?} {:?} {:?} {:?} {:?} {limits:?}",
            s.policy.local_only,
            s.tiers,
            s.auto,
            s.spend,
            s.describe_images,
            s.my_network,
            s.agent_endpoint
        ),
        rejected,
    )
}

fn file_with(path: &str, value: toml::Value) -> String {
    let mut table = toml::Table::new();
    let mut at = &mut table;
    let parts: Vec<&str> = path.split('.').collect();
    for part in &parts[..parts.len() - 1] {
        at = at
            .entry((*part).to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .expect("table");
    }
    at.insert(parts[parts.len() - 1].to_owned(), value);
    toml::to_string(&table).expect("toml")
}

#[test]
fn the_schema_holds_the_rows_the_design_names_and_each_is_a_page_row_of_intelligence() {
    let keys = schema_keys();
    let paths: Vec<&str> = keys.iter().map(|k| k.path.0.as_str()).collect();
    let mut want: Vec<String> = [
        "ai.local_only",
        "ai.attached.my_network",
        "ai.agents.endpoint",
    ]
    .map(String::from)
    .to_vec();
    want.extend(CLASSES.iter().map(|c| format!("ai.floor.{}", slug_of(c))));
    want.extend(
        [
            "ai.auto.mode",
            "ai.auto.allow_evict",
            "ai.auto.show_reason",
            "ai.spend.warn_permille",
            "ai.spend.account_daily_cents",
            "ai.spend.account_monthly_cents",
            "ai.spend.app_daily_cents",
            "ai.spend.app_monthly_cents",
            "ai.structured.open_text",
            "ai.structured.open_list",
            "ai.structured.depth",
            "ai.structured.repair_budget",
            "ai.pipeline.describe_images",
        ]
        .map(String::from),
    );
    assert_eq!(paths, want);
    for key in &keys {
        assert_eq!(key.page, Page::Intelligence, "{}", key.path.0);
        let want_section = match key.path.0.as_str() {
            p if p.starts_with("ai.structured.") => "Structured output",
            p if p.starts_with("ai.auto.") || p.starts_with("ai.spend.") => "Automatic",
            _ => "Models",
        };
        assert_eq!(key.section.0, want_section, "{}", key.path.0);
        let advanced = want_section != "Models";
        assert_eq!(
            key.exposure,
            if advanced {
                Exposure::Advanced
            } else {
                Exposure::Basic
            },
            "{}",
            key.path.0
        );
    }
}

#[test]
fn the_structured_rows_keep_the_codes_defaults_and_ranges() {
    use inferd::structured::limits::{DEPTH, OPEN_LIST, OPEN_TEXT, REPAIR_BUDGET};
    let keys = schema_keys();
    for row in [OPEN_TEXT, OPEN_LIST, DEPTH, REPAIR_BUDGET] {
        let key = keys.iter().find(|k| k.path.0 == row.path).expect(row.path);
        assert_eq!(key.default, toml::Value::Integer(i64::from(row.default)));
        assert_eq!(
            key.kind,
            KeyKind::Bounded {
                min: i64::from(*row.range.start()),
                max: i64::from(*row.range.end()),
                unit: key.kind_unit(),
            }
        );
    }
}

trait Unit {
    fn kind_unit(&self) -> Option<String>;
}
impl Unit for KeySpec {
    fn kind_unit(&self) -> Option<String> {
        match &self.kind {
            KeyKind::Bounded { unit, .. } => unit.clone(),
            _ => None,
        }
    }
}

#[test]
fn the_spend_row_is_bounded_one_to_a_thousand_and_defaults_to_the_codes_line() {
    let keys = schema_keys();
    let key = keys
        .iter()
        .find(|k| k.path.0 == settings::SPEND_WARN)
        .expect("row");
    assert_eq!(
        key.kind,
        KeyKind::Bounded {
            min: i64::from(*settings::SPEND_WARN_RANGE.start()),
            max: i64::from(*settings::SPEND_WARN_RANGE.end()),
            unit: key.kind_unit(),
        }
    );
    assert_eq!(
        key.default,
        toml::Value::Integer(i64::from(settings::SPEND_WARN_DEFAULT))
    );
}

#[test]
fn the_cap_rows_are_cents_from_zero_and_default_to_no_cap() {
    let keys = schema_keys();
    for path in [
        settings::SPEND_ACCOUNT_DAILY,
        settings::SPEND_ACCOUNT_MONTHLY,
        settings::SPEND_APP_DAILY,
        settings::SPEND_APP_MONTHLY,
    ] {
        let key = keys.iter().find(|k| k.path.0 == path).expect(path);
        assert_eq!(
            key.kind,
            KeyKind::Bounded {
                min: i64::from(*settings::SPEND_CAP_RANGE.start()),
                max: i64::from(*settings::SPEND_CAP_RANGE.end()),
                unit: Some("cents".into()),
            },
            "{path}"
        );
        assert_eq!(key.default, toml::Value::Integer(0), "{path}");
        // Zero is no cap, and a number is that many cents in micro-dollars.
        assert_eq!(
            effect(&file_with(path, toml::Value::Integer(0))).0,
            effect("").0
        );
    }
    assert_eq!(
        settings::cents_to_limit(250),
        Some(porter_core::MicroUsd(2_500_000))
    );
    assert_eq!(settings::cents_to_limit(0), None);
}

#[test]
fn every_ai_row_is_hands_off() {
    // design/22 section 9.7: `ai.` is in AGENT_NEVER_SETTABLE; a row marked settable would be
    // refused by the schema loader (and `schema()` above parses through it).
    for key in schema_keys() {
        assert!(key.path.0.starts_with("ai."));
        assert_eq!(key.agent, AgentSetting::HandsOff, "{}", key.path.0);
    }
    assert!(
        !dist("inferd.settings.toml")
            .lines()
            .any(|line| line.trim_start().starts_with("agent"))
    );
}

/// A value of the row other than its default, when it has one.
fn other_than_default(key: &KeySpec) -> Option<toml::Value> {
    let text = |v: &str| toml::Value::String(v.to_owned());
    match &key.kind {
        KeyKind::Toggle { variants } => variants
            .iter()
            .find(|v| toml::Value::String((*v).clone()) != key.default)
            .map(|v| text(v)),
        KeyKind::Segmented { variants } | KeyKind::Menu { variants } => variants
            .iter()
            .find(|v| toml::Value::String((*v).clone()) != key.default)
            .map(|v| text(v)),
        KeyKind::Bounded { min, max, .. } => Some(toml::Value::Integer(
            if key.default == toml::Value::Integer(*min) {
                *max
            } else {
                *min
            },
        )),
        _ => None,
    }
}

#[test]
fn every_row_of_the_schema_is_read_by_inferd_at_its_path_and_changes_what_it_does() {
    let nothing = effect("");
    assert_eq!(nothing.1, Vec::<String>::new());
    for key in schema_keys() {
        let path = key.path.0.as_str();
        // Its default, written at its path, is accepted and is what no file says.
        let said_default = effect(&file_with(path, key.default.clone()));
        assert_eq!(said_default.1, Vec::<String>::new(), "{path} default");
        assert_eq!(
            said_default.0, nothing.0,
            "{path}: the schema default is the code's"
        );
        match other_than_default(&key) {
            Some(value) => {
                let changed = effect(&file_with(path, value.clone()));
                assert_eq!(changed.1, Vec::<String>::new(), "{path} = {value}");
                assert_ne!(changed.0, nothing.0, "{path} = {value} changes nothing");
            }
            None => {
                // A row with one value (`ai.auto.mode`): inferd still reads it at its path, and
                // names it when it holds anything else.
                let refused = effect(&file_with(path, toml::Value::String("bogus".into())));
                assert_eq!(refused.1, vec![path.to_owned()], "{path}");
            }
        }
    }
}

#[test]
fn every_model_row_the_live_module_can_describe_is_read_at_its_path() {
    let nothing = effect("");
    for kind in settings::SLOTS {
        for tier in settings::TIERS {
            let path = settings::model_path(kind, tier);
            for value in ["auto", "local/some-model"] {
                let got = effect(&file_with(&path, toml::Value::String(value.into())));
                assert_eq!(got.1, Vec::<String>::new(), "{path} = {value}");
                assert_ne!(got.0, nothing.0, "{path} = {value}");
            }
            let empty = effect(&file_with(&path, toml::Value::String(String::new())));
            assert_eq!(empty.0, nothing.0, "{path}: empty is the catalogue default");
        }
    }
}

#[test]
fn the_sample_configuration_names_the_settings_app_and_its_example_rows_are_accepted() {
    let text = dist("inferd.toml");
    let config = InferdConfig::from_toml(&text).expect("the sample reads");
    // Settings is its unit; an app scope of its name is an app like any other (sec-3).
    let app = porter_core::AppName::parse("org.quire.Settings").expect("name");
    assert_eq!(
        config.callers.rows().role_of(&app),
        porter_dbus::CallerRole::App
    );
    assert_eq!(
        config
            .callers
            .resolve("org.quire.Settings.service")
            .map(|caller| caller.role),
        Some(inferd::peers::Role::Settings)
    );
    // The commented examples of the ai rows, uncommented, are accepted as they stand.
    let start = text.find("# [ai]\n").expect("the ai example");
    let end = text.find("# The old shape").expect("its end");
    let example: String = text[start..end]
        .lines()
        .filter_map(|line| line.strip_prefix("# "))
        .filter(|line| {
            line.starts_with('[')
                || line
                    .split_once(" = ")
                    .is_some_and(|(key, _)| key.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        })
        .map(|line| format!("{line}\n"))
        .collect();
    let got = effect(&example);
    assert_eq!(got.1, Vec::<String>::new(), "{example}");
    assert_ne!(got, effect(""), "the example says something");
}

#[test]
fn the_sample_configuration_never_enables_the_prompt_recorder() {
    let live: Vec<_> = dist("inferd.toml")
        .lines()
        .filter(|line| line.trim_start().starts_with("record"))
        .map(str::to_owned)
        .collect();
    assert!(live.is_empty(), "{live:?}");
}

#[test]
fn the_test_proc_root_is_in_no_default_feature_and_no_unit() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("manifest");
    let manifest: toml::Table = manifest.parse().expect("toml");
    let features = manifest["features"].as_table().expect("features");
    let default = features.get("default").and_then(toml::Value::as_array);
    assert!(
        default.is_none_or(|on| on.iter().all(|f| f.as_str() != Some("test-proc-root"))),
        "default features must not enable test-proc-root"
    );
    for file in [
        "inferd.service",
        "inferd.toml",
        "dbus/org.quire.Inference1.service",
    ] {
        assert!(!dist(file).contains("INFERD_PROC_ROOT"), "{file}");
    }
}
