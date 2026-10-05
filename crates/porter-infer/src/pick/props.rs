use super::tests::{ASK, cand, granted, mref, open};
use super::*;
use crate::policy::{ClassFloor, Floor, LocalOnly};
use crate::route::TierChoice;
use crate::spend::SpendVerdict;
use porter_core::consent::Verdict;
use porter_core::{AccountId, Billing, DataClass, Locality, ModelId};
use proptest::prelude::*;

fn readiness() -> impl Strategy<Value = Readiness> {
    prop_oneof![
        Just(Readiness::Ready),
        Just(Readiness::Loading),
        Just(Readiness::Loadable),
        Just(Readiness::Downloadable),
        Just(Readiness::Unavailable),
    ]
}

fn swap() -> impl Strategy<Value = SwapCost> {
    prop_oneof![
        Just(SwapCost::Resident),
        (0u16..400).prop_map(|s| SwapCost::Fits {
            cold_start_estimate_s: s
        }),
        (0u16..400, any::<bool>()).prop_map(|(s, idle)| SwapCost::Evicts {
            victim: mref("victim"),
            load: if idle {
                EngineLoad::Idle
            } else {
                EngineLoad::MidTurn
            },
            cold_start_estimate_s: s,
        }),
        (2u8..4).prop_map(|engines| SwapCost::Purge { engines }),
        Just(SwapCost::NoRoom),
    ]
}

fn locality() -> impl Strategy<Value = Locality> {
    prop_oneof![
        Just(Locality::OnDevice),
        Just(Locality::LocalNetwork),
        Just(Locality::Cloud { region: None }),
    ]
}

fn permission() -> impl Strategy<Value = Verdict> {
    prop_oneof![Just(granted()), Just(Verdict::Ask), Just(Verdict::Denied)]
}

fn spend() -> impl Strategy<Value = SpendVerdict> {
    prop_oneof![
        Just(SpendVerdict::Within),
        Just(SpendVerdict::Warn),
        Just(SpendVerdict::Stop)
    ]
}

/// Candidates with distinct names and catalogue indexes, in a shuffled slice order.
fn candidates() -> impl Strategy<Value = Vec<PickCandidate>> {
    let one = (
        readiness(),
        swap(),
        locality(),
        permission(),
        spend(),
        0u8..3,
        any::<bool>(),
    );
    proptest::collection::vec(one, 0..7)
        .prop_map(|rows| {
            rows.into_iter()
                .enumerate()
                .map(
                    |(i, (readiness, swap, locality, permission, spend, price, nc))| {
                        let mut c = cand(
                            &format!("m{i}"),
                            u32::try_from(i).expect("small"),
                            readiness,
                            swap,
                        );
                        c.route.locality = locality;
                        c.route.permission = permission;
                        c.route.spend = spend;
                        c.route.tier = TierChoice::Other;
                        c.route.billing = match price {
                            0 => Billing::Free,
                            1 => Billing::PlanBudget,
                            _ => Billing::Metered(porter_core::PriceTable {
                                input_per_mtok: porter_core::MicroUsd(u64::from(price)),
                                output_per_mtok: porter_core::MicroUsd(0),
                            }),
                        };
                        c.licence = if nc {
                            LicenceClass::NonCommercial
                        } else {
                            LicenceClass::Open
                        };
                        c
                    },
                )
                .collect::<Vec<_>>()
        })
        .prop_shuffle()
}

fn policy() -> impl Strategy<Value = Policy> {
    (any::<bool>(), 0u8..3).prop_map(|(local, floor)| Policy {
        local_only: if local { LocalOnly::On } else { LocalOnly::Off },
        floors: match floor {
            0 => vec![],
            1 => vec![ClassFloor {
                class: DataClass::Public,
                floor: Floor::OnDevice,
            }],
            _ => vec![ClassFloor {
                class: DataClass::Public,
                floor: Floor::LocalNetwork,
            }],
        },
    })
}

fn auto_policy() -> impl Strategy<Value = AutoPolicy> {
    (any::<bool>(), any::<bool>()).prop_map(|(never, off)| AutoPolicy {
        mode: AutoMode::WarmFirst,
        allow_evict: if never {
            AutoEvict::Never
        } else {
            AutoEvict::IdleOnly
        },
        show_reason: if off { ShowReason::Off } else { ShowReason::On },
    })
}

fn named_ref(i: usize) -> ModelRef {
    ModelRef {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse(&format!("m{i}")).expect("id"),
    }
}

proptest! {
    #[test]
    fn a_pick_is_deterministic_and_does_not_depend_on_the_slice_order(
        cands in candidates(), policy in policy(), auto in auto_policy(),
    ) {
        let rules = PickPolicy { policy: &policy, auto };
        let first = pick(ASK, &Pick::Auto(AutoMode::WarmFirst), &cands, rules);
        let again = pick(ASK, &Pick::Auto(AutoMode::WarmFirst), &cands, rules);
        prop_assert_eq!(&first, &again);
        let mut reversed = cands.clone();
        reversed.reverse();
        let other = pick(ASK, &Pick::Auto(AutoMode::WarmFirst), &reversed, rules);
        prop_assert_eq!(first, other);
    }

    #[test]
    fn automatic_never_returns_what_admit_refuses_or_what_the_rows_forbid(
        cands in candidates(), policy in policy(), auto in auto_policy(),
    ) {
        let rules = PickPolicy { policy: &policy, auto };
        if let Ok(got) = pick(ASK, &Pick::Auto(AutoMode::WarmFirst), &cands, rules) {
            let admitted = admit(ASK, cands.iter().map(|c| &c.route), &policy).expect("admit allows something");
            let found = cands
                .iter()
                .find(|c| c.route.account == got.chosen.account && c.route.model == got.chosen.model)
                .expect("a candidate");
            prop_assert!(admitted.iter().any(|a| std::ptr::eq(*a, &found.route)));
            prop_assert!(found.licence != LicenceClass::NonCommercial);
            prop_assert!(matches!(found.readiness, Readiness::Ready | Readiness::Loading | Readiness::Loadable));
            match &found.swap {
                SwapCost::Resident | SwapCost::Fits { .. } => {}
                SwapCost::Evicts { load, victim, .. } => {
                    prop_assert_eq!(*load, EngineLoad::Idle);
                    prop_assert_eq!(auto.allow_evict, AutoEvict::IdleOnly);
                    prop_assert_eq!(got.why, Why::Evicted { model: victim.clone() });
                }
                SwapCost::Purge { .. } | SwapCost::NoRoom => prop_assert!(false, "unloadable"),
            }
            if policy.local_only == LocalOnly::On {
                let cloud = matches!(found.route.locality, Locality::Cloud { .. });
                prop_assert!(!cloud);
            }
            prop_assert!(policy.floor(ASK.class).admits(&found.route.locality));
        }
    }

    #[test]
    fn a_named_model_is_served_or_refused_and_never_replaced(
        cands in candidates(), policy in policy(), auto in auto_policy(), which in 0usize..8,
    ) {
        let name = named_ref(which);
        let rules = PickPolicy { policy: &policy, auto };
        match pick(ASK, &Pick::Named(name.clone()), &cands, rules) {
            Ok(got) => {
                prop_assert_eq!(got.why, Why::Named);
                prop_assert_eq!(got.chosen.account, name.account);
                prop_assert_eq!(got.chosen.model, name.model);
            }
            Err(refusal) => {
                let declined = refusal.declined.expect("a named refusal names the model");
                prop_assert_eq!(declined.model, name);
            }
        }
    }

    #[test]
    fn an_open_policy_and_one_candidate_picks_it_whenever_it_can_answer(
        ready in readiness(), cost in swap(),
    ) {
        let one = cand("m0", 0, ready, cost.clone());
        let rules = PickPolicy { policy: &open(), auto: AutoPolicy::default() };
        let got = pick(ASK, &Pick::Auto(AutoMode::WarmFirst), &[one], rules);
        let answers = matches!(ready, Readiness::Ready | Readiness::Loading | Readiness::Loadable)
            && matches!(&cost, SwapCost::Resident | SwapCost::Fits { .. }
                | SwapCost::Evicts { load: EngineLoad::Idle, .. });
        prop_assert_eq!(got.is_ok(), answers);
    }
}
