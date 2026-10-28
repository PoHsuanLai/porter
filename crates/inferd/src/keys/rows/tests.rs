use super::*;
use porter_capability::StorageScope;
use porter_core::{DataClass, SpaceScope};
use porter_core::{GrantKey, consent::Usage};
use porter_fake::llm_account;
use porter_ids::UnixSeconds;

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
                reach: StorageScope::AppFolder,
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
fn every_row_path_reads_back_as_its_key_and_a_dotted_permit_id_is_found() {
    let registry = registry();
    let id = registry.accounts[0].id.clone();
    let grant = GrantId::parse("g.1").expect("id");
    let keys = [
        Key::Account(id.clone(), porter_settings::STATE),
        Key::Account(id.clone(), porter_settings::LABEL),
        Key::Account(id.clone(), porter_settings::PLACE),
        Key::Account(id.clone(), porter_settings::SIGN_IN),
        Key::Account(id.clone(), porter_settings::PROVIDER),
        Key::Account(id.clone(), porter_settings::GROUP),
        Key::Account(id.clone(), porter_settings::REAUTH),
        Key::SignOut(id.clone()),
        Key::Account(id.clone(), porter_settings::REMOVE),
        Key::Service(id.clone(), CapabilityKind::Llm),
        Key::Grant(id.clone(), grant.clone()),
        Key::Since(id.clone(), grant.clone()),
        Key::Scope(id.clone(), grant.clone()),
        Key::App(id.clone(), grant.clone()),
        Key::GrantService(id.clone(), grant),
        Key::SpendMonth(id.clone()),
        Key::SpendCap(id.clone()),
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
        true,
    );
    let paths: Vec<&str> = rows.iter().map(|r| r.path.0.as_str()).collect();
    for suffix in [
        "grant.g.1",
        "grant.g.1.since",
        "grant.g.1.scope",
        "grant.g.1.app",
        "grant.g.1.service",
        "remove",
        "label",
    ] {
        let want = format!("accounts.{}.{suffix}", registry.accounts[0].id);
        assert!(paths.contains(&want.as_str()), "{want} in {paths:?}");
    }
}

#[test]
fn a_permit_names_its_app_and_its_service_in_rows_of_their_own() {
    let registry = registry();
    let rows = schema(
        &registry,
        &AppNames::default(),
        &ProviderNames::default(),
        false,
    );
    let row = |suffix: &str| {
        let want = format!("accounts.{}.grant.g.1.{suffix}", registry.accounts[0].id);
        rows.iter()
            .find(|row| row.path.0 == want)
            .unwrap_or_else(|| panic!("{want}"))
            .clone()
    };
    let app = row("app");
    assert_eq!(
        app.default,
        toml::Value::String("org.quire.Companion".into())
    );
    assert!(
        app.labels.0.contains_key("org.quire.Companion"),
        "the app's name is a label: {:?}",
        app.labels
    );
    let service = row("service");
    assert_eq!(
        service.default,
        toml::Value::String(slug(&CapabilityKind::Llm))
    );
    assert_eq!(
        service
            .labels
            .0
            .get(&slug(&CapabilityKind::Llm))
            .map(String::as_str),
        Some(CapabilityKind::Llm.name())
    );
}

#[test]
fn only_an_account_over_the_network_has_spend_rows_and_only_when_inferd_meters() {
    let mut registry = registry();
    let mut hosted = registry.accounts[0].clone();
    hosted.id = AccountId::parse("hosted").expect("id");
    hosted.auth = AuthKind::ApiKey;
    registry.accounts[0].auth = AuthKind::LocalRuntime;
    registry.accounts.push(hosted);
    let spend_paths = |spending: bool| -> Vec<String> {
        schema(
            &registry,
            &AppNames::default(),
            &ProviderNames::default(),
            spending,
        )
        .into_iter()
        .map(|row| row.path.0)
        .filter(|path| path.contains(".spend."))
        .collect()
    };
    assert_eq!(
        spend_paths(true),
        ["accounts.hosted.spend.month", "accounts.hosted.spend.cap"]
    );
    assert!(spend_paths(false).is_empty());
}

#[test]
fn spend_is_whole_cents_and_a_part_of_a_cent_counts_as_one() {
    let usd = |millionths: i64| Money::usd(Decimal::new(millionths, 6));
    assert_eq!(cents_up(usd(0)), "0");
    assert_eq!(cents_up(usd(1)), "1");
    assert_eq!(cents_up(usd(10_000)), "1");
    assert_eq!(cents_up(usd(1_234_567)), "124");
    let euros = Money::new(Decimal::new(3, 0), Currency::EUR);
    assert_eq!(
        cents_up(euros),
        "3 EUR",
        "another currency is written as it is"
    );
}

/// Before any request runs, the limit row of an account whose provider reports no charges says
/// the limit cannot apply; OpenRouter, which reports each reply's charge, says nothing of the sort.
#[test]
fn the_limit_row_says_when_the_provider_reports_no_charges() {
    let mut registry = registry();
    let template = registry.accounts[0].clone();
    registry.accounts.clear();
    for (id, provider) in [("sees-nothing", "anthropic"), ("sees-all", "openrouter")] {
        let mut account = template.clone();
        account.id = AccountId::parse(id).expect("id");
        account.provider = porter_core::ProviderId::parse(provider).expect("provider");
        account.auth = AuthKind::ApiKey;
        registry.accounts.push(account);
    }
    let rows = schema(
        &registry,
        &AppNames::default(),
        &ProviderNames::default(),
        true,
    );
    let help = |path: &str| {
        rows.iter()
            .find(|row| row.path.0 == path)
            .unwrap_or_else(|| panic!("{path}"))
            .help
            .0
            .clone()
    };
    assert_eq!(
        help("accounts.sees-nothing.spend.cap"),
        "anthropic doesn't report what its replies cost, so a limit can't apply to it."
    );
    assert!(help("accounts.sees-nothing.spend.month").contains("nothing is counted"));
    assert_eq!(help("accounts.sees-all.spend.cap"), "");
    assert_eq!(help("accounts.sees-all.spend.month"), "");
}
