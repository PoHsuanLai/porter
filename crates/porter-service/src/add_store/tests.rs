use super::*;
use porter_core::{AccountLabel, AuthKind, Restriction};

fn account(id: &str) -> Account {
    Account {
        id: AccountId::parse(id).expect("id"),
        provider: ProviderId::parse("nextcloud").expect("id"),
        label: AccountLabel("x".into()),
        state: AccountState::Ok,
        auth: AuthKind::LoginFlowV2,
        capabilities: vec![],
        restriction: Restriction::none(),
        endpoints: vec![],
    }
}

fn registry(ids: &[&str]) -> Registry {
    Registry {
        accounts: ids.iter().map(|id| account(id)).collect(),
        ..Registry::default()
    }
}

#[test]
fn an_id_is_the_provider_and_the_label_as_a_slug_with_a_number_when_taken() {
    let provider = ProviderId::parse("nextcloud").expect("id");
    const CASES: &[(&str, &[&str], &str)] = &[
        ("an address", &[], "nextcloud-ada-example.org"),
        ("a port and uppercase", &[], "nextcloud-ada-example.org"),
        (
            "taken once",
            &["nextcloud-ada-example.org"],
            "nextcloud-ada-example.org-2",
        ),
        (
            "taken twice",
            &["nextcloud-ada-example.org", "nextcloud-ada-example.org-2"],
            "nextcloud-ada-example.org-3",
        ),
    ];
    let labels = [
        "Ada@Example.org",
        "ADA@example.org",
        "ada@example.org",
        "ada@example.org",
    ];
    for ((case, taken, want), label) in CASES.iter().zip(labels) {
        assert_eq!(
            fresh_id(&registry(taken), &provider, label).as_str(),
            *want,
            "{case}"
        );
    }
}

#[test]
fn a_long_label_is_cut_to_an_id_that_still_parses_and_stays_unique() {
    let provider = ProviderId::parse("nextcloud").expect("id");
    let label = format!("{}@example.org", "a".repeat(100));
    let first = fresh_id(&Registry::default(), &provider, &label);
    assert!(first.as_str().len() <= 64, "{first}");
    let second = fresh_id(&registry(&[first.as_str()]), &provider, &label);
    assert_ne!(first, second);
    assert!(second.as_str().len() <= 64, "{second}");
}

#[test]
fn a_label_of_symbols_still_makes_an_id() {
    let provider = ProviderId::parse("generic-imap").expect("id");
    assert_eq!(
        fresh_id(&Registry::default(), &provider, "@@@").as_str(),
        "generic-imap"
    );
}
