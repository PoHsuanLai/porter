use super::*;

fn table() -> CallerTable {
    CallerTable::from_toml_text(
        r#"
cua = ["/usr/libexec/quire/cuad"]
[apps]
"org.quire.Memory" = ["/usr/libexec/quire/memoryd"]
"org.quire.Mail" = ["/usr/bin/mailo", "/opt/mailo/mailo"]
"org.quire.Sneaky" = ["/usr/libexec/quire/cuad"]
"#,
    )
    .expect("table")
}

fn named(caller: &Caller) -> (&str, Role, Isolation) {
    (caller.app.name.as_str(), caller.role, caller.app.isolation)
}

#[test]
fn an_executable_is_the_caller_the_table_names_for_it() {
    let table = table();
    let memory = table
        .resolve(Path::new("/usr/libexec/quire/memoryd"))
        .expect("memoryd");
    assert_eq!(
        named(&memory),
        ("org.quire.Memory", Role::App, Isolation::Unsandboxed)
    );
    for exe in ["/usr/bin/mailo", "/opt/mailo/mailo"] {
        let mail = table.resolve(Path::new(exe)).expect("mailo");
        assert_eq!(mail.app.name.as_str(), "org.quire.Mail");
    }
}

#[test]
fn cuad_is_the_one_computer_use_caller_and_an_app_entry_cannot_claim_its_executable() {
    let cuad = table()
        .resolve(Path::new("/usr/libexec/quire/cuad"))
        .expect("cuad");
    assert_eq!(
        named(&cuad),
        ("org.quire.Cua", Role::Cua, Isolation::Unsandboxed)
    );
}

#[test]
fn a_program_the_table_does_not_name_is_nobody() {
    let table = table();
    for exe in ["/usr/bin/bash", "/usr/libexec/quire/memoryd (deleted)", ""] {
        assert_eq!(table.resolve(Path::new(exe)), None, "{exe:?}");
    }
    assert_eq!(
        CallerTable::default().resolve(Path::new("/usr/bin/mailo")),
        None
    );
}

#[test]
fn a_table_with_a_malformed_app_name_does_not_parse() {
    assert!(CallerTable::from_toml_text("[apps]\n\"not a name\" = [\"/x\"]\n").is_err());
    assert!(CallerTable::from_toml_text("cua = 3").is_err());
}

#[tokio::test]
async fn introduced_connections_are_known_and_others_are_not() {
    let peers = TablePeers::new();
    let caller = table().resolve(Path::new("/usr/bin/mailo")).expect("mailo");
    peers.introduce(":1.42", caller.clone());
    assert_eq!(peers.caller_of(":1.42").await, Some(caller.clone()));
    assert_eq!(peers.caller_of(":1.43").await, None);
    let shared = std::sync::Arc::new(peers);
    assert_eq!(shared.caller_of(":1.42").await, Some(caller));
}
