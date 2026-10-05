use super::*;
use porter_core::Isolation;

fn table() -> CallerTable {
    CallerTable::from_toml_text(
        r#"
cua = ["cuad.service"]
[apps]
"org.quire.Memory" = ["memoryd.service"]
"org.quire.Mail" = ["mailo.service", "mailo-dev.service"]
"org.quire.Sneaky" = ["cuad.service"]
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
    assert!(
        roles[1..]
            .iter()
            .all(|r| *r == porter_dbus::CallerRole::App)
    );
    assert_eq!(rows.callers.len(), 5);
    // The first row for cuad's unit is cuad's, not the sneaky app's.
    let cuad = rows.resolve_unit("cuad.service").expect("cuad");
    assert_eq!(cuad.role, porter_dbus::CallerRole::Cua);
}
