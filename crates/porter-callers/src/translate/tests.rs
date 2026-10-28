use super::super::{Install, installed};
use super::*;
use porter_core::AppName;

fn row(app: &str, unit: Option<&str>, role: CallerRole, exe: &[&str]) -> CallerRow {
    CallerRow {
        app: AppName::parse(app).expect("name"),
        unit: unit.map(str::to_owned),
        role,
        exe: exe.iter().map(|image| (*image).to_owned()).collect(),
        name: None,
    }
}

fn table_of(rows: Vec<CallerRow>) -> Table {
    peephole_table(&CallerTable { callers: rows })
}

fn unit(name: &str) -> peephole_core::UnitName {
    peephole_core::UnitName::new(name).expect("unit")
}

fn images_of(table: &Table, name: &str) -> Option<Vec<String>> {
    table
        .row_for_unit(&unit(name))
        .map(|row| row.exe.iter().map(|p| p.as_str().to_owned()).collect())
}

#[test]
fn every_role_round_trips_through_peepholes_spelling() {
    for role in [
        CallerRole::App,
        CallerRole::Settings,
        CallerRole::SheetHost,
        CallerRole::PorterDaemon,
        CallerRole::Agent,
        CallerRole::Cua,
        CallerRole::AgentLauncher,
        CallerRole::Terminal,
        CallerRole::Compositor,
    ] {
        let name = role_name(role).expect("a valid role name");
        assert_eq!(role_of_name(&name), Some(role), "{name}");
    }
    assert_eq!(role_of_name(&RoleName::from_static(MANAGER_ROLE)), None);
}

#[test]
fn a_daemon_unit_without_a_program_is_taken_to_be_installed_as_a_daemon() {
    let table = table_of(vec![row(
        "org.quire.Inference",
        Some("inferd.service"),
        CallerRole::PorterDaemon,
        &[],
    )]);
    let found = table.row_for_unit(&unit("inferd.service")).expect("a row");
    assert_eq!(found.app.as_str(), "org.quire.Inference");
    assert_eq!(found.role.as_str(), "porter_daemon");
    assert_eq!(
        images_of(&table, "inferd.service"),
        Some(installed(Install::Libexec, "inferd"))
    );
}

#[test]
fn a_rows_own_programs_replace_the_convention() {
    let table = table_of(vec![row(
        "org.quire.Shell",
        Some("sill.service"),
        CallerRole::SheetHost,
        &["/usr/bin/sill", "/opt/quire/releases/*/bin/sill"],
    )]);
    assert_eq!(
        images_of(&table, "sill.service"),
        Some(vec![
            "/usr/bin/sill".to_owned(),
            "/opt/quire/releases/*/bin/sill".to_owned()
        ])
    );
}

#[test]
fn a_row_peephole_could_not_judge_is_left_out_and_the_others_stand() {
    let table = table_of(vec![
        // An app by name: no unit.
        row("org.quire.Settings", None, CallerRole::Settings, &[]),
        // A unit that is not a daemon's name, and no program: nothing proves it.
        row(
            "org.quire.Temor",
            Some("org.quire.Temor.service"),
            CallerRole::Terminal,
            &[],
        ),
        // A program path peephole refuses (two stars in a segment).
        row(
            "org.quire.Bad",
            Some("bad.service"),
            CallerRole::Agent,
            &["/opt/*/*x*/bad"],
        ),
        row(
            "org.quire.Intents",
            Some("intentd.service"),
            CallerRole::Agent,
            &[],
        ),
    ]);
    assert!(
        table
            .row_for_unit(&unit("org.quire.Temor.service"))
            .is_none()
    );
    assert!(table.row_for_unit(&unit("bad.service")).is_none());
    assert!(table.row_for_unit(&unit("intentd.service")).is_some());
}

#[test]
fn the_first_row_for_a_unit_wins() {
    let table = table_of(vec![
        row("org.quire.Cua", Some("cuad.service"), CallerRole::Cua, &[]),
        row(
            "org.example.Mine",
            Some("cuad.service"),
            CallerRole::App,
            &[],
        ),
    ]);
    let found = table.row_for_unit(&unit("cuad.service")).expect("a row");
    assert_eq!(
        (found.app.as_str(), found.role.as_str()),
        ("org.quire.Cua", "cua")
    );
}

#[test]
fn the_table_always_holds_the_unit_managers_row() {
    let table = table_of(Vec::new());
    assert!(table.has_role(&RoleName::from_static(MANAGER_ROLE)));
    assert_eq!(table.rows().len(), 1);
}
