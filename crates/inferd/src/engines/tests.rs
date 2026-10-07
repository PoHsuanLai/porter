use super::*;
use crate::serve::Router;
use crate::supervise::Ports;
use crate::testkit::{Recorder, Scratch, models};
use engine_supervisor::{
    EngineFailure, EngineId, ExitCode, FakeGpu, FakeReadyProbe, GpuMemory, MonoMs, Probe,
    SupervisorConfig,
};
use model_catalog::MiB;
use porter_core::capability::{Capability, CuaEnv, LlmCap, LlmFeature, LlmWire, SpeechMode};
use porter_core::need::{CuaNeed, LlmNeed, SpeechNeed};
use porter_core::{AccountId, Billing, Locality, ModelId, Tokens};
use porter_infer::LocalOnly;

fn llm() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(1000),
    })
}

fn cua() -> Need {
    Need::ComputerUse(CuaNeed {
        environments: [CuaEnv::Desktop].into(),
    })
}

fn spec(need: Need, class: DataClass) -> SessionSpec {
    SessionSpec {
        need,
        class,
        tier: Tier::Balanced,
        usage: porter_core::consent::Usage::Interactive,
    }
}

fn start(scratch: &Scratch, host: Recorder) -> Engines {
    let models = models(scratch);
    let supervised = Supervised::start(
        models.iter().map(|m| m.spec.clone()).collect(),
        SupervisorConfig::default(),
        Ports {
            host,
            probe: FakeReadyProbe(Probe::Ready),
            gpu: FakeGpu(GpuMemory {
                total: MiB(16_000),
                used_by_others: MiB(0),
            }),
        },
    );
    Engines::new(models, supervised, Policy::proposed(), TierMap::default())
}

fn cloud() -> ModelCard {
    ModelCard {
        account: AccountId::parse("anthropic").expect("id"),
        model: ModelId::parse("sonnet").expect("id"),
        locality: Locality::Cloud { region: None },
        billing: Billing::PlanBudget,
        capabilities: vec![Capability::Llm(LlmCap {
            features: [LlmFeature::Chat].into(),
            context: Tokens(100_000),
            max_output: Tokens(4096),
            wire: LlmWire::Messages,
        })],
    }
}

fn snapshot(states: &[(&str, EngineState)], now: u64) -> Snapshot {
    Snapshot {
        states: states
            .iter()
            .map(|(id, state)| (EngineId((*id).into()), *state))
            .collect(),
        failures: Default::default(),
        now: Some(MonoMs(now)),
        gpu: None,
    }
}

#[test]
fn readiness_follows_the_engine_state_and_the_weights() {
    let scratch = Scratch::new("eng-ready");
    let models = models(&scratch);
    let model = &models[0];
    let id = model.spec.id.0.as_str();
    let ready = EngineState::Ready {
        since: MonoMs(0),
        last_used: MonoMs(0),
    };
    let starting = EngineState::Starting {
        since: MonoMs(0),
        attempt: engine_supervisor::Attempt(1),
    };
    let backoff = EngineState::Backoff {
        until: MonoMs(9),
        attempt: engine_supervisor::Attempt(2),
    };
    let cases = [
        (Some(ready), Readiness::Ready),
        (Some(starting), Readiness::Loading),
        (Some(backoff), Readiness::Loading),
        (Some(EngineState::Stopped), Readiness::Loadable),
        (
            Some(EngineState::Stopping { since: MonoMs(0) }),
            Readiness::Loadable,
        ),
        (
            Some(EngineState::Failed(EngineFailure::Exited {
                code: ExitCode(1),
            })),
            Readiness::Unavailable,
        ),
        (None, Readiness::Unavailable),
    ];
    for (state, expected) in cases {
        let states: Vec<(&str, EngineState)> = state.map(|s| (id, s)).into_iter().collect();
        assert_eq!(
            readiness_in(&snapshot(&states, 0), model),
            expected,
            "{state:?}"
        );
    }
    // Without weights nothing else matters.
    std::fs::remove_dir_all(model.spec.unit.sandbox.read.first().expect("bind")).expect("remove");
    assert_eq!(
        readiness_in(&snapshot(&[(id, ready)], 0), model),
        Readiness::Downloadable
    );
}

#[test]
fn the_gpu_is_loading_while_an_engine_starts_busy_in_a_turn_and_idle_otherwise() {
    let window = Duration::from_millis(500);
    let ready = |last_used| EngineState::Ready {
        since: MonoMs(0),
        last_used: MonoMs(last_used),
    };
    let starting = EngineState::Starting {
        since: MonoMs(0),
        attempt: engine_supervisor::Attempt(1),
    };
    let cases = [
        ("nothing", snapshot(&[], 10_000), GpuState::Idle),
        (
            "an engine just used",
            snapshot(&[("a", ready(9_800))], 10_000),
            GpuState::Busy,
        ),
        (
            "an engine long idle",
            snapshot(&[("a", ready(1_000))], 10_000),
            GpuState::Idle,
        ),
        (
            "one starting",
            snapshot(&[("a", starting)], 10_000),
            GpuState::Loading,
        ),
        (
            "loading outranks busy",
            snapshot(&[("a", ready(9_900)), ("b", starting)], 10_000),
            GpuState::Loading,
        ),
        (
            "not yet stepped",
            Snapshot {
                states: [(EngineId("a".into()), ready(0))].into(),
                failures: Default::default(),
                now: None,
                gpu: None,
            },
            GpuState::Idle,
        ),
    ];
    for (name, given, expected) in cases {
        assert_eq!(gpu_in(&given, window), expected, "{name}");
    }
    assert_eq!(
        [GpuState::Idle, GpuState::Busy, GpuState::Loading].map(GpuState::slug),
        ["idle", "busy", "loading"]
    );
}

#[tokio::test]
async fn an_empty_engines_routes_nothing_and_is_idle() {
    let engines = Engines::default();
    assert_eq!(
        engines
            .route(&spec(llm(), DataClass::Notes), Role::App)
            .err(),
        Some(InferRefusal::Unavailable)
    );
    assert_eq!(engines.gpu(), GpuState::Idle);
    assert!(engines.listed().is_empty());
    assert_eq!(
        engines.prepare(&llm(), DataClass::Notes, Tier::Fast).await,
        Err(InferRefusal::Unavailable)
    );
}

#[tokio::test(start_paused = true)]
async fn a_route_pins_the_model_and_the_engine_is_asked_for_through_the_host_seam() {
    let scratch = Scratch::new("eng-route");
    let host = Recorder::default();
    let engines = start(&scratch, host.clone());
    let (decision, pinned) = engines
        .route(&spec(llm(), DataClass::Notes), Role::App)
        .expect("route");
    assert_eq!(decision.served.model.as_str(), "tiny-chat");
    assert_eq!(decision.served.locality, Locality::OnDevice);
    assert_eq!(decision.readiness, Readiness::Loadable);
    assert_eq!(
        pinned.model.as_ref().map(|m| m.card.model.as_str()),
        Some("tiny-chat")
    );

    let model = ModelRef {
        account: decision.served.account.clone(),
        model: decision.served.model.clone(),
    };
    assert_eq!(engines.want(model.clone()).await, Ok(()));
    assert_eq!(host.spawned.lock().expect("lock").len(), 1);
    assert_eq!(
        engines
            .route(&spec(llm(), DataClass::Notes), Role::App)
            .expect("route")
            .0
            .readiness,
        Readiness::Ready
    );
    // Releasing keeps the engine: it unloads on its idle timer.
    engines.release(&model);
    assert_eq!(
        engines
            .route(&spec(llm(), DataClass::Notes), Role::App)
            .expect("route")
            .0
            .readiness,
        Readiness::Ready
    );

    // A model that is not ours to start fails.
    let other = ModelRef {
        account: AccountId::parse("anthropic").expect("id"),
        model: ModelId::parse("sonnet").expect("id"),
    };
    assert_eq!(engines.want(other).await, Err(EngineFailed::unknown()));
}

#[tokio::test(start_paused = true)]
async fn the_session_router_writes_the_pin_the_runner_reads() {
    let scratch = Scratch::new("eng-pin");
    let engines = start(&scratch, Recorder::default());
    let pin = Pin::new();
    let router = engines.router(pin.clone(), Role::App);
    let decision = router
        .route(&spec(llm(), DataClass::Notes))
        .await
        .expect("route");
    assert_eq!(decision.served.model.as_str(), "tiny-chat");
    assert_eq!(
        pin.get().map(|p| p.served.model.as_str().to_owned()),
        Some("tiny-chat".to_owned())
    );
    // A refusal leaves the cell empty.
    let empty = Pin::new();
    let none = Engines::default().router(empty.clone(), Role::App);
    assert_eq!(
        none.route(&spec(llm(), DataClass::Notes)).await.err(),
        Some(InferRefusal::Unavailable)
    );
    assert!(empty.get().is_none());
}

#[tokio::test(start_paused = true)]
async fn prepare_warms_a_stopped_engine_and_a_later_prepare_says_ready() {
    let scratch = Scratch::new("eng-prepare");
    let host = Recorder::default();
    let engines = start(&scratch, host.clone());
    assert_eq!(
        engines.prepare(&llm(), DataClass::Notes, Tier::Fast).await,
        Ok(Readiness::Loading)
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(host.spawned.lock().expect("lock").len(), 1);
    assert_eq!(
        engines.prepare(&llm(), DataClass::Notes, Tier::Fast).await,
        Ok(Readiness::Ready)
    );
    assert_eq!(
        host.spawned.lock().expect("lock").len(),
        1,
        "warming a ready engine starts nothing"
    );
    // Refusals come back as refusals.
    assert_eq!(
        engines.prepare(&cua(), DataClass::Screen, Tier::Fast).await,
        Err(InferRefusal::Denied),
        "an app may not warm a computer-use model"
    );
    assert_eq!(
        engines
            .prepare_as(&cua(), DataClass::Screen, Tier::Fast, Role::Cua)
            .await,
        Ok(Readiness::Loading)
    );
}

#[tokio::test(start_paused = true)]
async fn only_cuad_is_routed_a_computer_use_need() {
    let scratch = Scratch::new("eng-cua");
    let engines = start(&scratch, Recorder::default());
    assert_eq!(
        engines
            .route(&spec(cua(), DataClass::Screen), Role::App)
            .err(),
        Some(InferRefusal::Denied)
    );
    let (decision, _) = engines
        .route(&spec(cua(), DataClass::Screen), Role::Cua)
        .expect("route");
    assert_eq!(decision.served.model.as_str(), "tiny-cua");
}

#[tokio::test(start_paused = true)]
async fn what_an_app_is_told_without_a_session_reveals_no_identity() {
    let scratch = Scratch::new("eng-avail");
    let engines = start(&scratch, Recorder::default());
    let speech = Need::Speech(SpeechNeed {
        modes: [SpeechMode::Stt].into(),
    });
    let cases = [
        (
            "a model here",
            llm(),
            DataClass::Notes,
            Role::App,
            Availability::Granted,
        ),
        (
            "computer use for an app",
            cua(),
            DataClass::Screen,
            Role::App,
            Availability::Denied,
        ),
        (
            "computer use for cuad",
            cua(),
            DataClass::Screen,
            Role::Cua,
            Availability::Granted,
        ),
        (
            "computer use on mail",
            cua(),
            DataClass::Mail,
            Role::Cua,
            Availability::Unsupported,
        ),
        (
            "speech has no runner",
            speech,
            DataClass::Voice,
            Role::App,
            Availability::NeedsAccount,
        ),
    ];
    for (name, need, class, role, expected) in cases {
        assert_eq!(engines.availability(&need, class, role), expected, "{name}");
    }

    // Accounts that are not here: a floor that forbids them, or a grant nobody gave.
    let mut policy = Policy::proposed();
    policy.local_only = LocalOnly::Off;
    let remote = Engines::new(Vec::new(), Supervised::idle(), policy, TierMap::default())
        .with_remote(vec![cloud()]);
    assert_eq!(
        remote.availability(&llm(), DataClass::Prompt, Role::App),
        Availability::NeedsAccount
    );
    assert_eq!(
        remote.availability(&llm(), DataClass::Public, Role::App),
        Availability::AvailableNeedsConsent
    );
    assert_eq!(remote.listed().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_route_says_why_and_the_snapshot_keeps_the_gpu_the_swap_cost_reads() {
    let scratch = Scratch::new("eng-why");
    let engines = start(&scratch, Recorder::default());
    let (decision, _) = engines
        .route_detailed(&spec(llm(), DataClass::Notes), Role::App)
        .expect("route");
    assert_eq!(decision.why, porter_infer::Why::CatalogueOrder);
    assert_eq!(decision.show, porter_infer::ShowReason::On);
    // Nothing has looked at the GPU yet; a cold model is then costed as fitting.
    assert_eq!(engines.supervised().snapshot().gpu, None);
    assert!(matches!(
        engines.listed()[0].swap,
        porter_infer::SwapCost::Fits { .. }
    ));
    let model = ModelRef {
        account: decision.served.account,
        model: decision.served.model,
    };
    assert_eq!(engines.want(model).await, Ok(()));
    assert_eq!(
        engines.supervised().snapshot().gpu,
        Some(GpuMemory {
            total: MiB(16_000),
            used_by_others: MiB(0)
        })
    );
    // Loaded now: nothing to swap.
    assert_eq!(engines.listed()[0].swap, porter_infer::SwapCost::Resident);
}
