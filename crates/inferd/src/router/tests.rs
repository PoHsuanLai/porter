use super::*;
use porter_core::capability::{
    Capability, CuaBatching, CuaCap, CuaEnv, LlmCap, LlmFeature, LlmWire, Offered,
};
use porter_core::need::{CuaNeed, LlmNeed};
use porter_core::{AccountId, Billing, ModelId, Px, Tokens};
use porter_infer::{LocalOnly, TierRow};

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
    Listed { card, readiness }
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
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(1000),
    })
}

fn cua_need() -> Need {
    Need::ComputerUse(CuaNeed {
        environments: [CuaEnv::Desktop].into(),
    })
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
    Need::Identity(porter_core::need::IdentityNeed {
        profile: Offered::Absent,
        verified_address: Offered::Absent,
    })
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
    )
    .map(|(chosen, readiness)| (chosen.model.as_str().to_owned(), readiness))
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
            Need::Llm(LlmNeed {
                features: [LlmFeature::Vision].into(),
                context: Tokens(1000),
            }),
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
            kind: AiKind::Llm,
            tier: Tier::Best,
            model: ModelRef {
                account: AccountId::parse("local").expect("id"),
                model: ModelId::parse(model).expect("id"),
            },
        }],
    };
    let route = |tier: Tier, map: &TierMap| {
        choose(
            &need(),
            DataClass::Notes,
            tier,
            &models,
            &Policy::proposed(),
            map,
        )
        .map(|(chosen, _)| chosen.model.as_str().to_owned())
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
    let speech = |modes: &[SpeechMode]| {
        Need::Speech(SpeechNeed {
            modes: modes.iter().copied().collect(),
        })
    };
    let cases = [
        (need(), Some(AiKind::Llm)),
        (cua_need(), Some(AiKind::ComputerUse)),
        (
            Need::Embeddings(EmbedNeed {
                dims: DimsNeed::Any,
                modalities: Default::default(),
            }),
            Some(AiKind::Embeddings),
        ),
        (speech(&[SpeechMode::Stt]), Some(AiKind::SpeechIn)),
        (speech(&[SpeechMode::Tts]), Some(AiKind::SpeechOut)),
        (
            speech(&[SpeechMode::Stt, SpeechMode::Tts]),
            Some(AiKind::SpeechOut),
        ),
        (identity(), None),
    ];
    for (need, kind) in cases {
        assert_eq!(ai_kind(&need), kind);
    }
}
