use super::*;

fn row(app: &str, unit: Option<&str>, role: CallerRole) -> CallerRow {
    CallerRow {
        app: AppName::parse(app).expect("name"),
        unit: unit.map(str::to_owned),
        role,
    }
}

fn name(text: &str) -> AppName {
    AppName::parse(text).expect("name")
}

fn table() -> CallerTable {
    CallerTable {
        callers: vec![
            row(
                "org.quire.Inference",
                Some("inferd.service"),
                CallerRole::PorterDaemon,
            ),
            row("org.quire.Settings", None, CallerRole::Settings),
            row("org.quire.Cua", Some("cuad.service"), CallerRole::Cua),
        ],
    }
}

#[test]
fn a_unit_resolves_to_its_app_and_role() {
    let cases = [
        (
            "inferd.service",
            Some(("org.quire.Inference", CallerRole::PorterDaemon)),
        ),
        ("cuad.service", Some(("org.quire.Cua", CallerRole::Cua))),
        // A row without a unit names no unit.
        ("detent.service", None),
        ("org.quire.Settings", None),
    ];
    for (unit, want) in cases {
        let got = table()
            .resolve_unit(unit)
            .map(|c| (c.app.name.to_string(), c.role));
        assert_eq!(got, want.map(|(n, r)| (n.to_owned(), r)), "{unit}");
    }
    assert_eq!(
        table()
            .resolve_unit("inferd.service")
            .map(|c| c.app.isolation),
        Some(Isolation::Unsandboxed)
    );
}

#[test]
fn an_apps_role_is_its_row_s_and_app_for_an_unlisted_app() {
    assert_eq!(
        table().role_of(&name("org.quire.Settings")),
        CallerRole::Settings
    );
    assert_eq!(table().role_of(&name("org.example.Any")), CallerRole::App);
}

#[test]
fn a_scope_named_after_a_units_app_gets_no_role_of_the_unit() {
    for app in ["org.quire.Inference", "org.quire.Cua"] {
        assert_eq!(table().role_of(&name(app)), CallerRole::App, "{app}");
    }
}

#[test]
fn a_user_row_replaces_the_system_row_of_the_same_unit_or_app() {
    let user = CallerTable {
        callers: vec![
            row(
                "org.example.MyInferd",
                Some("inferd.service"),
                CallerRole::App,
            ),
            row("org.quire.Settings", None, CallerRole::App),
        ],
    };
    let merged = CallerTable::layered(table(), user);
    let unit = merged.resolve_unit("inferd.service").expect("named");
    assert_eq!(
        (unit.app.name.as_str(), unit.role),
        ("org.example.MyInferd", CallerRole::App)
    );
    assert_eq!(merged.role_of(&name("org.quire.Settings")), CallerRole::App);
    assert_eq!(merged.callers.len(), 3);
}

#[test]
fn a_table_round_trips_through_its_serde_form() {
    let json = serde_json::to_string(&table()).expect("json");
    assert_eq!(
        serde_json::from_str::<CallerTable>(&json).expect("table"),
        table()
    );
    assert!(json.contains(r#""role":"porter_daemon""#));
}

#[test]
fn a_row_without_a_unit_deserializes() {
    let table: CallerTable =
        serde_json::from_str(r#"{"caller":[{"app":"org.quire.Settings","role":"settings"}]}"#)
            .expect("table");
    assert_eq!(table.callers[0].unit, None);
}
