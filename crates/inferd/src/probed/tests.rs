use super::*;
use crate::engines::Engines;
use crate::probe::testing::{chat, found};
use crate::serve::EngineHost;
use crate::session::SessionSpec;
use porter_core::capability::LlmFeature;
use porter_core::need::LlmNeed;
use porter_core::{DataClass, Locality, ModelId, Need, Tier, Tokens};
use porter_infer::{AutoMode, AutoRow, InferRefusal, LocalOnly, Policy, Slot, TierMap, TierRow};
use std::path::Path;

fn llm() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(1000),
    })
}

fn spec(class: DataClass) -> SessionSpec {
    SessionSpec {
        need: llm(),
        class,
        tier: Tier::Balanced,
        usage: porter_core::consent::Usage::Interactive,
    }
}

fn models_of(runtime: Runtime, port: u16, names: &[&str]) -> Vec<LocalModel> {
    let models = names
        .iter()
        .map(|name| chat(name, name, 8192, &[LlmFeature::Chat]))
        .collect();
    found(runtime, port, models).local_models(Path::new("/nonexistent"))
}

fn engines() -> Engines {
    Engines::default()
}

fn account(text: &str) -> AccountId {
    AccountId::parse(text).expect("id")
}

#[test]
fn a_runtime_that_answers_lists_its_models_ready_under_this_computer() {
    let engines = engines();
    engines.probed().set(
        Runtime::Ollama,
        Standing::Online,
        Some(models_of(
            Runtime::Ollama,
            11_434,
            &["llama3.2-3b", "qwen2.5-7b"],
        )),
    );
    let listed = engines.listed();
    let rows: Vec<_> = listed
        .iter()
        .map(|one| {
            (
                one.card.account.as_str(),
                one.card.model.as_str(),
                one.readiness,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("ollama", "llama3.2-3b", Readiness::Ready),
            ("ollama", "qwen2.5-7b", Readiness::Ready)
        ]
    );
    assert!(
        listed
            .iter()
            .all(|one| one.card.locality == Locality::OnDevice)
    );
    // A local model carries the on-this-computer grant: nobody is asked.
    assert!(listed.iter().all(|one| matches!(
        one.permission,
        porter_core::consent::Verdict::Granted { .. }
    )));
}

#[test]
fn a_runtime_that_stops_keeps_its_models_listed_with_nothing_to_serve_them_and_comes_back_as_it_was()
 {
    let engines = engines();
    let book = engines.probed();
    book.set(
        Runtime::Ollama,
        Standing::Online,
        Some(models_of(Runtime::Ollama, 11_434, &["llama3.2-3b"])),
    );
    book.set(Runtime::Ollama, Standing::Offline, None);
    let listed = engines.listed();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].readiness, Readiness::Unavailable);
    assert_eq!(
        book.standing_of(&account("ollama")),
        Some(Standing::Offline)
    );
    // Back, with no new list: the old one stands until a look says otherwise.
    book.set(Runtime::Ollama, Standing::Online, None);
    assert_eq!(engines.listed()[0].readiness, Readiness::Ready);
}

#[test]
fn a_new_look_replaces_the_models_of_that_runtime_and_no_other() {
    let engines = engines();
    let book = engines.probed();
    book.set(
        Runtime::Ollama,
        Standing::Online,
        Some(models_of(Runtime::Ollama, 1, &["a-model"])),
    );
    book.set(
        Runtime::LmStudio,
        Standing::Online,
        Some(models_of(Runtime::LmStudio, 2, &["b-model"])),
    );
    book.set(
        Runtime::Ollama,
        Standing::Online,
        Some(models_of(Runtime::Ollama, 1, &["c-model"])),
    );
    let ids: Vec<_> = book
        .models()
        .iter()
        .map(|(model, _)| format!("{}/{}", model.card.account, model.card.model))
        .collect();
    assert_eq!(ids, ["ollama/c-model", "lm-studio/b-model"]);
    let wanted = porter_infer::ModelRef {
        account: account("lm-studio"),
        model: ModelId::parse("b-model").expect("id"),
    };
    assert!(book.find(&wanted).is_some());
    assert_eq!(book.standing_of(&account("local")), None);
}

#[test]
fn the_default_route_serves_a_prompt_from_a_runtime_that_is_up() {
    let engines = engines();
    engines.probed().set(
        Runtime::Ollama,
        Standing::Online,
        Some(models_of(Runtime::Ollama, 11_434, &["llama3.2-3b"])),
    );
    let (routing, pinned) = engines
        .route_detailed(&spec(DataClass::Prompt), crate::peers::Role::App)
        .expect("a route");
    assert_eq!(routing.served.account.as_str(), "ollama");
    assert_eq!(routing.served.locality, Locality::OnDevice);
    assert_eq!(routing.readiness, Readiness::Ready);
    let model = pinned.model.expect("a local model to run");
    assert_eq!(model.loopback, Some(model_http::Port(11_434)));
}

#[test]
fn a_runtime_that_is_down_is_unavailable_and_never_replaced_by_a_remote_model() {
    let engines = engines().with_remote(vec![crate::testkit::cloud_card()]);
    engines.probed().set(
        Runtime::Ollama,
        Standing::Online,
        Some(models_of(Runtime::Ollama, 11_434, &["llama3.2-3b"])),
    );
    engines
        .probed()
        .set(Runtime::Ollama, Standing::Offline, None);
    let refused = engines
        .route(&spec(DataClass::Prompt), crate::peers::Role::App)
        .expect_err("nothing serves");
    assert_eq!(refused, InferRefusal::Unavailable);
}

#[test]
fn local_only_on_leaves_the_runtime_the_only_answer() {
    let policy = Policy {
        local_only: LocalOnly::On,
        ..Policy::proposed()
    };
    let engines = Engines::new(
        Vec::new(),
        crate::supervise::Supervised::idle(),
        policy,
        TierMap::default(),
    )
    .with_remote(vec![crate::testkit::cloud_card()]);
    engines.probed().set(
        Runtime::LlamaCpp,
        Standing::Online,
        Some(models_of(Runtime::LlamaCpp, 8080, &["gemma-3-4b"])),
    );
    let (routing, _) = engines
        .route_detailed(&spec(DataClass::Mail), crate::peers::Role::App)
        .expect("a route");
    assert_eq!(routing.served.account.as_str(), "llama-cpp");
    engines
        .probed()
        .set(Runtime::LlamaCpp, Standing::Offline, None);
    assert_eq!(
        engines
            .route(&spec(DataClass::Mail), crate::peers::Role::App)
            .expect_err("local only and nothing local"),
        InferRefusal::Unavailable
    );
}

#[test]
fn a_probed_model_is_eligible_for_auto_and_for_a_named_pick() {
    let model = models_of(Runtime::Ollama, 11_434, &["llama3.2-3b"]);
    let named = TierMap {
        rows: vec![TierRow {
            kind: Slot::Text,
            tier: Tier::Balanced,
            model: model[0].model_ref(),
        }],
        autos: Vec::new(),
    };
    let auto = TierMap {
        rows: Vec::new(),
        autos: vec![AutoRow {
            kind: Slot::Text,
            tier: Tier::Balanced,
            mode: AutoMode::WarmFirst,
        }],
    };
    for tiers in [named, auto] {
        let engines = Engines::new(
            Vec::new(),
            crate::supervise::Supervised::idle(),
            Policy::proposed(),
            tiers,
        );
        engines.probed().set(
            Runtime::Ollama,
            Standing::Online,
            Some(models_of(Runtime::Ollama, 11_434, &["llama3.2-3b"])),
        );
        let (routing, _) = engines
            .route_detailed(&spec(DataClass::Prompt), crate::peers::Role::App)
            .expect("a route");
        assert_eq!(routing.served.model.as_str(), "llama3.2-3b");
    }
}

#[tokio::test]
async fn a_session_may_start_on_a_runtime_that_is_up_and_not_on_one_that_is_not() {
    let engines = engines();
    let book = engines.probed();
    let model = models_of(Runtime::Ollama, 11_434, &["llama3.2-3b"]);
    let wanted = model[0].model_ref();
    book.set(Runtime::Ollama, Standing::Online, Some(model));
    assert!(engines.want(wanted.clone()).await.is_ok());
    book.set(Runtime::Ollama, Standing::Offline, None);
    assert!(engines.want(wanted).await.is_err());
}

#[tokio::test]
async fn every_look_wakes_whoever_tells_listeners() {
    let book = ProbedBook::default();
    let woken = book.changed();
    book.set(Runtime::Ollama, Standing::Online, None);
    tokio::time::timeout(porter_fake::GENEROUS, woken.notified())
        .await
        .expect("woken by the look that was recorded before anyone waited");
}
