use super::*;

fn row(app: &str, unit: Option<&str>, role: CallerRole) -> CallerRow {
    CallerRow {
        app: AppName::parse(app).expect("name"),
        unit: unit.map(str::to_owned),
        role,
        name: None,
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

#[test]
fn the_last_row_with_a_name_names_the_app_whatever_its_unit() {
    let named = |app: &str, unit: Option<&str>, title: &str| CallerRow {
        name: Some(AppTitle(title.to_owned())),
        ..row(app, unit, CallerRole::App)
    };
    let table = CallerTable {
        callers: vec![
            named("org.quire.Sync", Some("syncd.service"), "Sync"),
            row("org.quire.Settings", None, CallerRole::Settings),
            named("org.quire.Sync", None, "Mine"),
            row("org.quire.Sync", None, CallerRole::App),
        ],
    };
    assert_eq!(
        table.title_of(&name("org.quire.Sync")),
        Some(&AppTitle("Mine".to_owned()))
    );
    assert_eq!(table.title_of(&name("org.quire.Settings")), None);
    assert_eq!(table.title_of(&name("org.quire.Nobody")), None);
}

#[test]
fn a_user_row_cannot_claim_a_fixed_role_for_a_new_app_or_a_unit() {
    let user = CallerTable {
        callers: vec![
            row(
                "org.example.Mine",
                Some("mine.service"),
                CallerRole::SheetHost,
            ),
            row("org.example.Other", None, CallerRole::Settings),
            row(
                "org.example.Fake",
                Some("sill.service"),
                CallerRole::SheetHost,
            ),
        ],
    };
    assert_eq!(CallerTable::layered(table(), user), table());
}

#[test]
fn a_user_row_cannot_raise_a_system_app_to_a_fixed_role() {
    let system = CallerTable {
        callers: vec![row("org.example.Notes", None, CallerRole::App)],
    };
    let user = CallerTable {
        callers: vec![row("org.example.Notes", None, CallerRole::Cua)],
    };
    let merged = CallerTable::layered(system, user);
    assert_eq!(merged.role_of(&name("org.example.Notes")), CallerRole::App);
}

#[test]
fn a_user_row_cannot_replace_a_fixed_system_row_with_another_fixed_role() {
    let user = CallerTable {
        callers: vec![row(
            "org.example.Mine",
            Some("cuad.service"),
            CallerRole::PorterDaemon,
        )],
    };
    let merged = CallerTable::layered(table(), user);
    assert_eq!(
        merged.resolve_unit("cuad.service").map(|c| c.role),
        Some(CallerRole::Cua)
    );
}

#[test]
fn a_user_row_can_still_add_a_plain_app_or_an_agent() {
    let user = CallerTable {
        callers: vec![
            row("org.example.Plain", None, CallerRole::App),
            row(
                "org.example.Helper",
                Some("helper.service"),
                CallerRole::Agent,
            ),
        ],
    };
    let merged = CallerTable::layered(table(), user);
    assert_eq!(merged.callers.len(), 5);
    assert_eq!(merged.role_of(&name("org.example.Plain")), CallerRole::App);
    assert_eq!(
        merged.resolve_unit("helper.service").map(|c| c.role),
        Some(CallerRole::Agent)
    );
}

/// An app the system's table names is who its system rows say: a user row cannot give it
/// another unit or a row without one, though it may still lower one of its own rows.
#[test]
fn a_user_row_cannot_name_an_app_the_system_owns() {
    let system = || {
        let mut system = table();
        system.callers.push(row(
            "org.quire.Intents",
            Some("intentd.service"),
            CallerRole::Agent,
        ));
        system
    };
    // (case, the user's row, what intentd.service and helper.service resolve to after)
    type Case<'a> = (
        &'a str,
        CallerRow,
        Option<(&'a str, CallerRole)>,
        Option<(&'a str, CallerRole)>,
    );
    let intents = Some(("org.quire.Intents", CallerRole::Agent));
    let cases: [Case; 4] = [
        (
            "another unit as the agent",
            row(
                "org.quire.Intents",
                Some("helper.service"),
                CallerRole::Agent,
            ),
            intents,
            None,
        ),
        (
            "another unit as an app",
            row("org.quire.Intents", Some("helper.service"), CallerRole::App),
            intents,
            None,
        ),
        (
            "a row without a unit",
            row("org.quire.Intents", None, CallerRole::Agent),
            intents,
            None,
        ),
        (
            "its own unit, lowered",
            row(
                "org.quire.Intents",
                Some("intentd.service"),
                CallerRole::App,
            ),
            Some(("org.quire.Intents", CallerRole::App)),
            None,
        ),
    ];
    for (case, mine, intentd, helper) in cases {
        let merged = CallerTable::layered(
            system(),
            CallerTable {
                callers: vec![mine],
            },
        );
        let found = |unit: &str| {
            merged
                .resolve_unit(unit)
                .map(|c| (c.app.name.to_string(), c.role))
        };
        let want = |w: Option<(&str, CallerRole)>| w.map(|(n, r)| (n.to_owned(), r));
        assert_eq!(found("intentd.service"), want(intentd), "{case}");
        assert_eq!(found("helper.service"), want(helper), "{case}");
        assert_eq!(
            merged.role_of(&name("org.quire.Intents")),
            CallerRole::App,
            "{case}"
        );
    }
}
