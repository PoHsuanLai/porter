//! A local runtime reported as an account: made from its provider file, models as claims, offline
//! and back, never deleted; and what a runtime may not say.

use porter_core::capability::{Capability, LlmCap, LlmFeature, LlmWire};
use porter_core::{
    AbsentReason, AccountId, AccountState, Claim, ModelId, Offer, Provenance, ProviderId, Subject,
    Tokens,
};
use porter_fake::{FixedClock, MemoryStore, ScriptedSheets, cloud_provider, llm_provider};
use porter_secrets::MemorySecrets;
use porter_service::{AccountService, LocalFault, MAX_LOCAL_CLAIMS, Registry};

type Svc = AccountService<
    porter_fake::FakeProvider,
    MemorySecrets,
    ScriptedSheets,
    FixedClock,
    MemoryStore,
>;

fn service() -> (Svc, MemoryStore) {
    let store = MemoryStore::default();
    let service = AccountService::new(
        vec![llm_provider(), cloud_provider()],
        Registry::default(),
        MemorySecrets::default(),
        ScriptedSheets::default(),
        FixedClock(porter_fake::NOW),
    )
    .with_store(store.clone());
    (service, store)
}

fn runtime() -> ProviderId {
    ProviderId::parse("fake-llm").expect("id")
}

fn claim(model: &str, provenance: Provenance) -> Claim {
    Claim {
        subject: Subject::Model(ModelId::parse(model).expect("id")),
        offer: Offer::Present(Capability::Llm(LlmCap {
            features: [LlmFeature::Chat].into(),
            context: Tokens(4096),
            max_output: Tokens(4096),
            wire: LlmWire::ChatCompletions,
        })),
        provenance,
    }
}

fn discovered(model: &str) -> Claim {
    claim(model, Provenance::Discovered)
}

fn models(service: &Svc) -> Vec<String> {
    service
        .registry()
        .accounts
        .iter()
        .flat_map(|a| &a.capabilities)
        .filter_map(|c| match &c.subject {
            Subject::Model(id) => Some(id.to_string()),
            Subject::Account => None,
        })
        .collect()
}

#[tokio::test]
async fn the_first_report_makes_the_account_from_the_provider_file_and_saves_it() {
    let (service, store) = service();
    let id = service
        .report_local(&runtime(), vec![discovered("a-model")], AccountState::Ok)
        .await
        .expect("reported");
    assert_eq!(id, AccountId::parse("fake-llm").expect("id"));
    let registry = service.registry();
    let made = &registry.accounts[0];
    assert_eq!(
        (made.provider.as_str(), made.label.0.as_str(), made.state),
        ("fake-llm", "Fake Runtime", AccountState::Ok)
    );
    assert_eq!(models(&service), ["a-model"]);
    assert_eq!(store.stored().expect("saved").accounts.len(), 1);
}

#[tokio::test]
async fn a_second_report_updates_the_one_account_and_offline_keeps_the_models() {
    let (service, _) = service();
    service
        .report_local(&runtime(), vec![discovered("a-model")], AccountState::Ok)
        .await
        .expect("up");
    service
        .report_local(&runtime(), vec![], AccountState::Offline)
        .await
        .expect("down");
    let registry = service.registry();
    assert_eq!(registry.accounts.len(), 1);
    assert_eq!(registry.accounts[0].state, AccountState::Offline);
    assert_eq!(models(&service), ["a-model"]);

    // An `ok` report with no models is an empty runtime: the old list is gone.
    service
        .report_local(&runtime(), vec![], AccountState::Ok)
        .await
        .expect("empty");
    assert_eq!(service.registry().accounts[0].state, AccountState::Ok);
    assert_eq!(models(&service), Vec::<String>::new());
}

#[tokio::test]
async fn a_report_that_is_not_a_runtimes_to_make_is_refused_and_changes_nothing() {
    let (service, store) = service();
    let mut absent = discovered("a-model");
    absent.offer = Offer::Absent {
        kind: porter_core::CapabilityKind::Llm,
        reason: AbsentReason::TurnedOff,
    };
    let mut about_account = discovered("a-model");
    about_account.subject = Subject::Account;
    let many: Vec<Claim> = (0..=MAX_LOCAL_CLAIMS)
        .map(|n| discovered(&format!("m{n}")))
        .collect();
    let cloud = ProviderId::parse("fake-cloud").expect("id");
    let nobody = ProviderId::parse("nobody").expect("id");
    let table = [
        (
            &runtime(),
            vec![],
            AccountState::NeedsReauth,
            LocalFault::State,
        ),
        (&runtime(), vec![], AccountState::Limited, LocalFault::State),
        (
            &runtime(),
            vec![absent],
            AccountState::Ok,
            LocalFault::Claim,
        ),
        (
            &runtime(),
            vec![about_account],
            AccountState::Ok,
            LocalFault::Claim,
        ),
        (
            &runtime(),
            vec![claim("a-model", Provenance::Declared)],
            AccountState::Ok,
            LocalFault::Claim,
        ),
        (
            &runtime(),
            vec![claim("a-model", Provenance::Probed)],
            AccountState::Ok,
            LocalFault::Claim,
        ),
        (&runtime(), many, AccountState::Ok, LocalFault::Claim),
        (&cloud, vec![], AccountState::Ok, LocalFault::NotLocal),
        (
            &nobody,
            vec![],
            AccountState::Ok,
            LocalFault::UnknownProvider,
        ),
    ];
    for (provider, claims, state, want) in table {
        assert_eq!(
            service.report_local(provider, claims, state).await,
            Err(want),
            "{provider:?} {state:?}"
        );
    }
    assert!(service.registry().accounts.is_empty());
    assert_eq!(store.saves(), 0);
}

#[tokio::test]
async fn a_registry_that_cannot_be_saved_says_unavailable_and_keeps_the_change_in_memory() {
    let (service, store) = service();
    store.refusing(true);
    assert_eq!(
        service
            .report_local(&runtime(), vec![discovered("a-model")], AccountState::Ok)
            .await,
        Err(LocalFault::Unavailable)
    );
    assert_eq!(service.registry().accounts.len(), 1);
}
