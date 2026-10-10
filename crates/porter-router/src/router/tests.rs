use super::*;
use porter_core::capability::{
    Capability, CuaBatching, CuaCap, CuaEnv, LlmCap, LlmFeature, LlmWire, Offered,
};
use porter_core::need::{CuaNeed, LlmNeed};
use porter_core::{AccountId, Billing, ModelId, Px, Tokens};
use porter_infer::{AutoMode, AutoRow, Declined, DeclinedBecause, LocalOnly, TierRow};

fn llm_cap() -> Capability {
    Capability::Llm(LlmCap {
        features: [LlmFeature::Chat, LlmFeature::Tools].into(),
        context: Tokens(8192),
        max_output: Tokens(1024),
        wire: LlmWire::ChatCompletions,
    })
}

fn card(account: &str, model: &str, locality: Locality, capability: Capability) -> ModelCard {
    ModelCard {
        account: AccountId::parse(account).expect("id"),
        model: ModelId::parse(model).expect("id"),
        locality,
        billing: Billing::Free,
        capabilities: vec![capability],
    }
}

fn listed(card: ModelCard, readiness: Readiness) -> Listed {
    Listed::new(card, readiness, SwapCost::Resident, LicenceClass::Open)
}

fn local(model: &str) -> ModelCard {
    card("local", model, Locality::OnDevice, llm_cap())
}

fn cloud(model: &str) -> ModelCard {
    card(
        "anthropic",
        model,
        Locality::Cloud { region: None },
        llm_cap(),
    )
}

fn need() -> Need {
    Need::Llm(LlmNeed::new([LlmFeature::Chat].into(), Tokens(1000)))
}

fn cua_need() -> Need {
    Need::ComputerUse(CuaNeed::new([CuaEnv::Desktop].into()))
}

fn cua_cap() -> Capability {
    Capability::ComputerUse(CuaCap {
        environments: [CuaEnv::Desktop].into(),
        batching: CuaBatching::One,
        zoom: Offered::Absent,
        max_image: Px(1024),
        wire: LlmWire::ChatCompletions,
    })
}

fn identity() -> Need {
    Need::Identity(porter_core::need::IdentityNeed::new(
        Offered::Absent,
        Offered::Absent,
    ))
}

fn off() -> Policy {
    Policy {
        local_only: LocalOnly::Off,
        ..Policy::proposed()
    }
}

fn pick(
    need: &Need,
    class: DataClass,
    models: &[Listed],
    policy: &Policy,
) -> Result<(String, Readiness), InferRefusal> {
    choose(
        need,
        class,
        Tier::Balanced,
        models,
        policy,
        &TierMap::default(),
        AutoPolicy::default(),
    )
    .map(|d| (d.chosen.model.as_str().to_owned(), d.readiness))
    .map_err(|r| r.refusal)
}

#[test]
fn the_route_table() {
    let ready = Readiness::Ready;
    #[allow(clippy::type_complexity)]
    let cases: Vec<(
        &str,
        Need,
        DataClass,
        Vec<Listed>,
        Policy,
        Result<(&str, Readiness), InferRefusal>,
    )> = vec![
        (
            "a local model that fits",
            need(),
            DataClass::Notes,
            vec![listed(local("a"), ready)],
            Policy::proposed(),
            Ok(("a", ready)),
        ),
        (
            "readiness rides along",
            need(),
            DataClass::Notes,
            vec![listed(local("a"), Readiness::Loadable)],
            Policy::proposed(),
            Ok(("a", Readiness::Loadable)),
        ),
        (
            "nothing installed",
            need(),
            DataClass::Notes,
            vec![],
            Policy::proposed(),
            Err(InferRefusal::Unavailable),
        ),
        (
            "weights not downloaded cannot answer",
            need(),
            DataClass::Notes,
            vec![listed(local("a"), Readiness::Downloadable)],
            Policy::proposed(),
            Err(InferRefusal::Unavailable),
        ),
        (
            "a failed engine cannot answer",
            need(),
            DataClass::Notes,
            vec![listed(local("a"), Readiness::Unavailable)],
            Policy::proposed(),
            Err(InferRefusal::Unavailable),
        ),
        (
            "local-only removes the cloud",
            need(),
            DataClass::Notes,
            vec![listed(cloud("c"), ready)],
            Policy::proposed(),
            Err(InferRefusal::Unavailable),
        ),
        (
            "the prompt class is refused a cloud-only route by its floor",
            need(),
            DataClass::Prompt,
            vec![listed(cloud("c"), ready)],
            off(),
            Err(InferRefusal::RequiresCloud(DataClass::Prompt)),
        ),
        (
            "so is every personal class",
            need(),
            DataClass::Mail,
            vec![listed(cloud("c"), ready)],
            off(),
            Err(InferRefusal::RequiresCloud(DataClass::Mail)),
        ),
        (
            "public data may go to a cloud that nobody has granted yet",
            need(),
            DataClass::Public,
            vec![listed(cloud("c"), ready)],
            off(),
            Err(InferRefusal::NeedsGrant),
        ),
        (
            "this computer is preferred to the cloud",
            need(),
            DataClass::Public,
            vec![listed(cloud("c"), ready), listed(local("a"), ready)],
            off(),
            Ok(("a", ready)),
        ),
        (
            "a prompt goes to the local model beside a cloud one",
            need(),
            DataClass::Prompt,
            vec![listed(cloud("c"), ready), listed(local("a"), ready)],
            off(),
            Ok(("a", ready)),
        ),
        (
            "a need no model meets",
            Need::Llm(LlmNeed::new([LlmFeature::Vision].into(), Tokens(1000))),
            DataClass::Notes,
            vec![listed(local("a"), ready)],
            Policy::proposed(),
            Err(InferRefusal::Unavailable),
        ),
        (
            "computer use takes screen data only",
            cua_need(),
            DataClass::Mail,
            vec![listed(
                card("local", "u", Locality::OnDevice, cua_cap()),
                ready,
            )],
            Policy::proposed(),
            Err(InferRefusal::Unsupported),
        ),
        (
            "computer use on a screen",
            cua_need(),
            DataClass::Screen,
            vec![listed(
                card("local", "u", Locality::OnDevice, cua_cap()),
                ready,
            )],
            Policy::proposed(),
            Ok(("u", ready)),
        ),
        (
            "a need that is not AI",
            identity(),
            DataClass::Notes,
            vec![listed(local("a"), ready)],
            Policy::proposed(),
            Err(InferRefusal::Unsupported),
        ),
    ];
    for (name, need, class, models, policy, expected) in cases {
        let expected = expected.map(|(model, readiness)| (model.to_owned(), readiness));
        assert_eq!(pick(&need, class, &models, &policy), expected, "{name}");
    }
}

#[test]
fn the_users_tier_choice_decides_between_models_that_fit() {
    let models = [
        listed(local("alpha"), Readiness::Ready),
        listed(local("beta"), Readiness::Ready),
    ];
    let tiers = |model: &str| TierMap {
        rows: vec![TierRow {
            kind: Slot::Text,
            tier: Tier::Best,
            model: ModelRef {
                account: AccountId::parse("local").expect("id"),
                model: ModelId::parse(model).expect("id"),
            },
        }],
        autos: vec![],
    };
    let route = |tier: Tier, map: &TierMap| {
        choose(
            &need(),
            DataClass::Notes,
            tier,
            &models,
            &Policy::proposed(),
            map,
            AutoPolicy::default(),
        )
        .map(|d| d.chosen.model.as_str().to_owned())
        .map_err(|r| r.refusal)
    };
    assert_eq!(route(Tier::Best, &tiers("beta")).as_deref(), Ok("beta"));
    assert_eq!(route(Tier::Best, &tiers("alpha")).as_deref(), Ok("alpha"));
    // The choice is for one tier only; elsewhere the first listed wins the tie.
    assert_eq!(route(Tier::Fast, &tiers("beta")).as_deref(), Ok("alpha"));
}

#[test]
fn needs_map_to_the_picker_kinds() {
    use porter_core::capability::SpeechMode;
    use porter_core::need::{DimsNeed, EmbedNeed, SpeechNeed};
    let speech =
        |modes: &[SpeechMode]| Need::Speech(SpeechNeed::new(modes.iter().copied().collect()));
    let cases = [
        (need(), Some(Slot::Text)),
        (cua_need(), Some(Slot::ComputerUse)),
        (
            Need::Embeddings(EmbedNeed::new(DimsNeed::Any, Default::default())),
            Some(Slot::Embeddings),
        ),
        (speech(&[SpeechMode::Stt]), Some(Slot::VoiceIn)),
        (speech(&[SpeechMode::Tts]), Some(Slot::VoiceOut)),
        (
            speech(&[SpeechMode::Stt, SpeechMode::Tts]),
            Some(Slot::VoiceOut),
        ),
        (identity(), None),
    ];
    for (need, kind) in cases {
        assert_eq!(slot_of_need(&need), kind);
    }
}

fn named(model: &str) -> ModelRef {
    ModelRef {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse(model).expect("id"),
    }
}

fn decide(models: &[Listed], tiers: &TierMap, auto: AutoPolicy) -> Result<Decided, PickRefusal> {
    choose(
        &need(),
        DataClass::Notes,
        Tier::Balanced,
        models,
        &Policy::proposed(),
        tiers,
        auto,
    )
}

fn auto_map() -> TierMap {
    TierMap {
        rows: vec![],
        autos: vec![AutoRow {
            kind: Slot::Text,
            tier: Tier::Balanced,
            mode: AutoMode::WarmFirst,
        }],
    }
}

#[test]
fn an_empty_row_says_why_in_catalogue_terms() {
    let one = [listed(local("alpha"), Readiness::Loadable)];
    let two = [
        listed(local("alpha"), Readiness::Loadable),
        listed(local("beta"), Readiness::Ready),
    ];
    let none = TierMap::default();
    let only = decide(&one, &none, AutoPolicy::default()).expect("one");
    assert_eq!(only.why, Why::OnlyOne);
    let first = decide(&two, &none, AutoPolicy::default()).expect("first");
    assert_eq!(
        (first.chosen.model.as_str(), first.why),
        ("alpha", Why::CatalogueOrder)
    );
}

#[test]
fn automatic_prefers_the_loaded_model_and_says_so() {
    let models = [
        listed(local("alpha"), Readiness::Loadable),
        listed(local("beta"), Readiness::Ready),
    ];
    let got = decide(&models, &auto_map(), AutoPolicy::default()).expect("picks");
    assert_eq!((got.chosen.model.as_str(), got.why), ("beta", Why::Warm));
    assert_eq!(got.readiness, Readiness::Ready);
}

#[test]
fn automatic_names_the_idle_model_it_unloads_and_only_when_allowed() {
    let swapping = Listed {
        swap: SwapCost::Evicts {
            victim: named("old"),
            load: porter_infer::EngineLoad::Idle,
            cold_start_estimate_s: 90,
        },
        ..listed(local("alpha"), Readiness::Loadable)
    };
    let got = decide(
        std::slice::from_ref(&swapping),
        &auto_map(),
        AutoPolicy::default(),
    )
    .expect("swaps");
    assert_eq!(
        got.why,
        Why::Evicted {
            model: named("old")
        }
    );
    let never = AutoPolicy {
        allow_evict: porter_infer::AutoEvict::Never,
        ..AutoPolicy::default()
    };
    assert_eq!(
        decide(&[swapping], &auto_map(), never)
            .expect_err("no swap")
            .refusal,
        InferRefusal::Unavailable
    );
}

#[test]
fn automatic_takes_the_cloud_only_when_the_floor_and_local_only_allow() {
    let models = [listed(cloud("claude"), Readiness::Ready)];
    // Notes data stays on this computer by default: not even Automatic widens the floor.
    assert_eq!(
        decide(&models, &auto_map(), AutoPolicy::default())
            .expect_err("floor")
            .refusal,
        InferRefusal::Unavailable
    );
    let open = choose(
        &need(),
        DataClass::Public,
        Tier::Balanced,
        &models,
        &off(),
        &auto_map(),
        AutoPolicy::default(),
    )
    .expect_err("a cloud model needs a grant, which only accountd can give and cannot yet");
    // `admit` lets Public data reach the cloud; consent is the rule that stops it today.
    assert_eq!(open.refusal, InferRefusal::NeedsGrant);
}

#[test]
fn a_named_model_that_cannot_serve_refuses_naming_it_and_no_other_answers() {
    let models = [
        listed(local("alpha"), Readiness::Ready),
        listed(local("beta"), Readiness::Downloadable),
    ];
    let tiers = TierMap {
        rows: vec![TierRow {
            kind: Slot::Text,
            tier: Tier::Balanced,
            model: named("beta"),
        }],
        autos: vec![],
    };
    let err = decide(&models, &tiers, AutoPolicy::default()).expect_err("refuses");
    assert_eq!(err.refusal, InferRefusal::Unavailable);
    assert_eq!(
        err.declined,
        Some(Declined {
            model: named("beta"),
            because: DeclinedBecause::NotInstalled
        })
    );
}
