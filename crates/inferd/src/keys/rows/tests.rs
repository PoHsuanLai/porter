use super::*;
use porter_core::UnixSeconds;
use porter_core::consent::{GrantKey, Usage};
use porter_core::{DataClass, SpaceScope};
use porter_fake::llm_account;

fn registry() -> Registry {
    let account = llm_account();
    Registry {
        grants: vec![Grant {
            id: GrantId::parse("g.1").expect("id"),
            key: GrantKey {
                app: AppId {
                    name: AppName::parse("org.quire.Companion").expect("name"),
                    isolation: Isolation::Unsandboxed,
                },
                account: account.id.clone(),
                kind: CapabilityKind::Llm,
                class: DataClass::Prompt,
                usage: Usage::Interactive,
                space: SpaceScope::Any,
            },
            decision: Decision::Allow,
            scope: GrantScope::Always,
            at: UnixSeconds(0),
        }],
        accounts: vec![account],
        toggles: vec![],
    }
}

#[test]
fn the_utc_day_of_a_unix_time() {
    let cases = [
        (0, "1970-01-01"),
        (86_400, "1970-01-02"),
        (951_782_400, "2000-02-29"),
        (-86_400, "1969-12-31"),
    ];
    for (at, want) in cases {
        assert_eq!(civil_date(UnixSeconds(at)), want, "{at}");
    }
}

#[test]
fn every_row_path_reads_back_as_its_key_and_a_dotted_permit_id_is_found() {
    let registry = registry();
    let id = registry.accounts[0].id.clone();
    let grant = GrantId::parse("g.1").expect("id");
    let keys = [
        Key::State(id.clone()),
        Key::Label(id.clone()),
        Key::Place(id.clone()),
        Key::SignIn(id.clone()),
        Key::Provider(id.clone()),
        Key::Group(id.clone()),
        Key::Reauth(id.clone()),
        Key::SignOut(id.clone()),
        Key::Remove(id.clone()),
        Key::Service(id.clone(), CapabilityKind::Llm),
        Key::Grant(id.clone(), grant.clone()),
        Key::Since(id.clone(), grant.clone()),
        Key::Scope(id.clone(), grant),
    ];
    for key in keys {
        assert_eq!(parse(&path(&key), &registry), Some(key.clone()), "{key:?}");
    }
    assert_eq!(parse("ai.model.text.fast", &registry), None);
    assert_eq!(parse("accounts.nobody.state", &registry), None);
}

#[test]
fn the_permit_of_an_app_is_a_revoke_row_with_its_day_and_how_long_it_lasts() {
    let registry = registry();
    let rows = schema(
        &registry,
        &AppNames::default(),
        &ProviderNames::default(),
    );
    let paths: Vec<&str> = rows.iter().map(|r| r.path.0.as_str()).collect();
    for suffix in ["grant.g.1", "grant.g.1.since", "grant.g.1.scope", "remove", "label"] {
        let want = format!("accounts.{}.{suffix}", registry.accounts[0].id);
        assert!(paths.contains(&want.as_str()), "{want} in {paths:?}");
    }
}
