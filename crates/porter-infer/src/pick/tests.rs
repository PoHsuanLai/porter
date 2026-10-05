use super::*;
use crate::policy::{ClassFloor, Floor, LocalOnly};
use crate::spend::SpendVerdict;
use porter_core::consent::{GrantScope, Verdict};
use porter_core::{
    AccountId, Billing, DataClass, GrantId, Locality, MicroUsd, ModelId, PriceTable,
};

pub(super) fn granted() -> Verdict {
    Verdict::Granted {
        grant: GrantId::parse("g").expect("id"),
        scope: GrantScope::Always,
    }
}

pub(super) fn mref(name: &str) -> ModelRef {
    ModelRef {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse(name).expect("id"),
    }
}

/// A local model in the catalogue at `index`, with the given readiness and swap cost.
pub(super) fn cand(name: &str, index: u32, readiness: Readiness, swap: SwapCost) -> PickCandidate {
    let model = mref(name);
    PickCandidate {
        route: RouteCandidate {
            account: model.account,
            model: model.model,
            locality: Locality::OnDevice,
            billing: Billing::Free,
            tier: crate::route::TierChoice::Other,
            permission: granted(),
            spend: SpendVerdict::Within,
        },
        readiness,
        swap,
        licence: LicenceClass::Open,
        catalogue_index: index,
    }
}

fn warm(name: &str, index: u32) -> PickCandidate {
    cand(name, index, Readiness::Ready, SwapCost::Resident)
}

fn cold(name: &str, index: u32, secs: u16) -> PickCandidate {
    cand(
        name,
        index,
        Readiness::Loadable,
        SwapCost::Fits {
            cold_start_estimate_s: secs,
        },
    )
}

fn evicting(name: &str, index: u32, victim: &str, load: EngineLoad) -> PickCandidate {
    cand(
        name,
        index,
        Readiness::Loadable,
        SwapCost::Evicts {
            victim: mref(victim),
            load,
            cold_start_estimate_s: 60,
        },
    )
}

fn cloud(name: &str, index: u32) -> PickCandidate {
    let mut c = cand(name, index, Readiness::Ready, SwapCost::Resident);
    c.route.locality = Locality::Cloud { region: None };
    c.route.billing = Billing::Metered(PriceTable {
        input_per_mtok: MicroUsd(3),
        output_per_mtok: MicroUsd(3),
    });
    c
}

pub(super) fn open() -> Policy {
    Policy {
        local_only: LocalOnly::Off,
        floors: vec![],
    }
}

pub(super) const ASK: RouteAsk = RouteAsk {
    class: DataClass::Public,
};

fn auto_with(
    cands: &[PickCandidate],
    policy: &Policy,
    allow_evict: AutoEvict,
) -> Result<Picked, PickRefusal> {
    let rules = PickPolicy {
        policy,
        auto: AutoPolicy {
            allow_evict,
            ..AutoPolicy::default()
        },
    };
    pick(ASK, &Pick::Auto(AutoMode::WarmFirst), cands, rules)
}

fn auto_of(cands: &[PickCandidate]) -> Result<Picked, PickRefusal> {
    auto_with(cands, &open(), AutoEvict::IdleOnly)
}

fn model_name(p: &Picked) -> String {
    p.chosen.model.to_string()
}

#[test]
fn automatic_table() {
    struct Case {
        name: &'static str,
        cands: Vec<PickCandidate>,
        policy: Policy,
        evict: AutoEvict,
        expected: Result<(&'static str, Why), InferRefusal>,
    }
    let case = |name, cands, expected| Case {
        name,
        cands,
        policy: open(),
        evict: AutoEvict::IdleOnly,
        expected,
    };
    let cases = vec![
        case(
            "the only candidate",
            vec![cold("a", 0, 30)],
            Ok(("a", Why::OnlyOne)),
        ),
        case(
            "a loaded model beats a cold one listed first",
            vec![cold("a", 0, 5), warm("b", 1)],
            Ok(("b", Why::Warm)),
        ),
        case(
            "a model that is loading counts as loaded",
            vec![
                cold("a", 0, 5),
                cand("b", 1, Readiness::Loading, SwapCost::Resident),
            ],
            Ok(("b", Why::Warm)),
        ),
        case(
            "the smaller cold start among cold models",
            vec![cold("a", 0, 90), cold("b", 1, 20)],
            Ok(("b", Why::Smallest)),
        ),
        case(
            "a model that fits beside beats one that needs a swap",
            vec![evicting("a", 0, "x", EngineLoad::Idle), cold("b", 1, 300)],
            Ok(("b", Why::Smallest)),
        ),
        case(
            "the catalogue order breaks a full tie",
            vec![warm("b", 1), warm("a", 0)],
            Ok(("a", Why::CatalogueOrder)),
        ),
        case(
            "this computer beats a loaded cloud model",
            vec![cloud("c", 0), cold("a", 1, 30)],
            Ok(("a", Why::Nearest)),
        ),
        case(
            "an idle engine is unloaded when nothing else can answer",
            vec![evicting("a", 0, "x", EngineLoad::Idle)],
            Ok(("a", Why::Evicted { model: mref("x") })),
        ),
        Case {
            evict: AutoEvict::Never,
            ..case(
                "never-evict leaves a swap alone",
                vec![evicting("a", 0, "x", EngineLoad::Idle)],
                Err(InferRefusal::Unavailable),
            )
        },
        Case {
            evict: AutoEvict::Never,
            ..case(
                "never-evict stays with the loaded model",
                vec![evicting("a", 0, "x", EngineLoad::Idle), warm("b", 1)],
                Ok(("b", Why::OnlyOne)),
            )
        },
        case(
            "an engine in a turn is never unloaded",
            vec![evicting("a", 0, "x", EngineLoad::MidTurn)],
            Err(InferRefusal::Unavailable),
        ),
        case(
            "no room is not a candidate",
            vec![cand("a", 0, Readiness::Loadable, SwapCost::NoRoom)],
            Err(InferRefusal::Unavailable),
        ),
        case(
            "unloading several engines is never done for the person",
            vec![cand(
                "a",
                0,
                Readiness::Loadable,
                SwapCost::Purge { engines: 2 },
            )],
            Err(InferRefusal::Unavailable),
        ),
        case(
            "a model that is not installed is not a candidate",
            vec![cand(
                "a",
                0,
                Readiness::Downloadable,
                SwapCost::Fits {
                    cold_start_estimate_s: 1,
                },
            )],
            Err(InferRefusal::Unavailable),
        ),
        case(
            "a non-commercial model is never chosen",
            vec![{
                let mut c = warm("a", 0);
                c.licence = LicenceClass::NonCommercial;
                c
            }],
            Err(InferRefusal::Unavailable),
        ),
        Case {
            policy: Policy::proposed(),
            ..case(
                "local-only drops a loaded cloud model",
                vec![cloud("c", 0), cold("a", 1, 30)],
                Ok(("a", Why::OnlyOne)),
            )
        },
        Case {
            policy: Policy::proposed(),
            ..case(
                "local-only with only cloud is unavailable",
                vec![cloud("c", 0)],
                Err(InferRefusal::Unavailable),
            )
        },
        Case {
            policy: Policy {
                local_only: LocalOnly::Off,
                floors: vec![ClassFloor {
                    class: DataClass::Public,
                    floor: Floor::OnDevice,
                }],
            },
            ..case(
                "a floor that forbids cloud is not widened",
                vec![cloud("c", 0)],
                Err(InferRefusal::RequiresCloud(DataClass::Public)),
            )
        },
        case(
            "the cloud when nothing nearer can answer and the rules allow it",
            vec![cloud("c", 0)],
            Ok(("c", Why::OnlyOne)),
        ),
    ];
    for c in cases {
        let got = auto_with(&c.cands, &c.policy, c.evict)
            .map(|p| (model_name(&p), p.why))
            .map_err(|r| r.refusal);
        let expected = c.expected.map(|(m, w)| (m.to_owned(), w));
        assert_eq!(got, expected, "{}", c.name);
    }
}

#[test]
fn automatic_unloads_idle_only_and_names_what_it_unloads() {
    let picked = auto_of(&[evicting("a", 0, "x", EngineLoad::Idle)]).expect("picks");
    assert_eq!(picked.readiness, Readiness::Loadable);
    assert_eq!(picked.why, Why::Evicted { model: mref("x") });
}

#[test]
fn a_mid_turn_engine_is_never_chosen_for_eviction_even_when_unloading_is_allowed() {
    let cands = [
        evicting("a", 0, "busy", EngineLoad::MidTurn),
        evicting("b", 1, "idle", EngineLoad::Idle),
    ];
    let picked = auto_of(&cands).expect("the idle one");
    assert_eq!(model_name(&picked), "b");
    assert_eq!(
        picked.why,
        Why::Evicted {
            model: mref("idle")
        }
    );
    assert!(auto_of(&cands[..1]).is_err());
}

fn named_pick(name: &str, cands: &[PickCandidate], policy: &Policy) -> Result<Picked, PickRefusal> {
    let rules = PickPolicy {
        policy,
        auto: AutoPolicy::default(),
    };
    pick(ASK, &Pick::Named(mref(name)), cands, rules)
}

#[test]
fn a_named_model_is_served_whatever_the_others_are() {
    let cands = [warm("b", 0), cold("a", 1, 200)];
    let picked = named_pick("a", &cands, &open()).expect("a");
    assert_eq!(model_name(&picked), "a");
    assert_eq!(picked.why, Why::Named);
    assert_eq!(picked.readiness, Readiness::Loadable);
}

#[test]
fn a_named_model_that_cannot_serve_refuses_and_says_why_without_a_fallback() {
    let warm_b = warm("b", 0);
    let policy_local = Policy::proposed();
    let table: Vec<(
        &str,
        Vec<PickCandidate>,
        Policy,
        DeclinedBecause,
        InferRefusal,
    )> = vec![
        (
            "not offered",
            vec![warm_b.clone()],
            open(),
            DeclinedBecause::NotListed,
            InferRefusal::Unavailable,
        ),
        (
            "not installed",
            vec![
                warm_b.clone(),
                cand(
                    "a",
                    1,
                    Readiness::Downloadable,
                    SwapCost::Fits {
                        cold_start_estimate_s: 1,
                    },
                ),
            ],
            open(),
            DeclinedBecause::NotInstalled,
            InferRefusal::Unavailable,
        ),
        (
            "failed",
            vec![
                warm_b.clone(),
                cand("a", 1, Readiness::Unavailable, SwapCost::Resident),
            ],
            open(),
            DeclinedBecause::Unavailable,
            InferRefusal::Unavailable,
        ),
        (
            "no room",
            vec![
                warm_b.clone(),
                cand("a", 1, Readiness::Loadable, SwapCost::NoRoom),
            ],
            open(),
            DeclinedBecause::NoRoom,
            InferRefusal::Unavailable,
        ),
        (
            "local-only refuses a named cloud model",
            vec![warm_b, cloud("a", 1)],
            policy_local,
            DeclinedBecause::Blocked {
                refusal: InferRefusal::Unavailable,
            },
            InferRefusal::Unavailable,
        ),
    ];
    for (name, cands, policy, because, refusal) in table {
        let err = named_pick("a", &cands, &policy).expect_err(name);
        assert_eq!(err.refusal, refusal, "{name}");
        assert_eq!(
            err.declined,
            Some(Declined {
                model: mref("a"),
                because
            }),
            "{name}"
        );
    }
}

#[test]
fn a_named_model_the_floor_forbids_is_blocked_by_the_floor() {
    let policy = Policy {
        local_only: LocalOnly::Off,
        floors: vec![ClassFloor {
            class: DataClass::Public,
            floor: Floor::OnDevice,
        }],
    };
    let err = named_pick("c", &[cloud("c", 0), warm("a", 1)], &policy).expect_err("floor");
    assert_eq!(err.refusal, InferRefusal::RequiresCloud(DataClass::Public));
}

#[test]
fn a_named_non_commercial_model_is_served_because_the_person_named_it() {
    let mut c = warm("a", 0);
    c.licence = LicenceClass::NonCommercial;
    assert!(named_pick("a", &[c], &open()).is_ok());
}

#[test]
fn the_vocabulary_has_stable_slugs() {
    assert_eq!(AutoMode::from_slug("warm_first"), Some(AutoMode::WarmFirst));
    assert_eq!(AutoEvict::from_slug("idle_only"), Some(AutoEvict::IdleOnly));
    assert_eq!(AutoEvict::from_slug("never"), Some(AutoEvict::Never));
    assert_eq!(ShowReason::from_slug("on"), Some(ShowReason::On));
    assert_eq!(ShowReason::from_slug("loud"), None);
    let defaults = AutoPolicy::default();
    assert_eq!(
        (defaults.mode, defaults.allow_evict, defaults.show_reason),
        (AutoMode::WarmFirst, AutoEvict::IdleOnly, ShowReason::On)
    );
    let why = serde_json::to_string(&Why::Evicted { model: mref("x") }).expect("json");
    assert_eq!(
        why,
        r#"{"kind":"evicted","model":{"account":"local","model":"x"}}"#
    );
    assert_eq!(
        serde_json::to_string(&Why::CatalogueOrder).expect("json"),
        r#"{"kind":"catalogue_order"}"#
    );
}

#[test]
fn no_why_is_a_ranking_word() {
    for why in [
        Why::Named,
        Why::OnlyOne,
        Why::Nearest,
        Why::Warm,
        Why::Smallest,
        Why::LeastCost,
        Why::CatalogueOrder,
    ] {
        let text = serde_json::to_string(&why).expect("json").to_lowercase();
        for word in ["best", "better", "smart", "optimal", "recommended"] {
            assert!(!text.contains(word), "{text}");
        }
    }
}
