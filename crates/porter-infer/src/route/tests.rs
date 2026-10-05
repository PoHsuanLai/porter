use super::*;
use crate::policy::{ClassFloor, Floor};
use porter_core::consent::GrantScope;
use porter_core::{GrantId, MicroUsd, PriceTable};

fn granted() -> Verdict {
    Verdict::Granted {
        grant: GrantId::parse("g").expect("id"),
        scope: GrantScope::Always,
    }
}

fn metered(input: u64) -> Billing {
    Billing::Metered(PriceTable {
        input_per_mtok: MicroUsd(input),
        output_per_mtok: MicroUsd(0),
    })
}

fn candidate(name: &str, locality: Locality, billing: Billing) -> RouteCandidate {
    RouteCandidate {
        account: AccountId::parse(name).expect("id"),
        model: ModelId::parse("m").expect("id"),
        locality,
        billing,
        tier: TierChoice::Chosen,
        permission: granted(),
        spend: SpendVerdict::Within,
    }
}

fn ollama() -> RouteCandidate {
    candidate("ollama", Locality::OnDevice, Billing::Free)
}
fn peer() -> RouteCandidate {
    candidate("peer", Locality::LocalNetwork, Billing::Free)
}
fn cloud(name: &str, input: u64) -> RouteCandidate {
    candidate(name, Locality::Cloud { region: None }, metered(input))
}

fn open_policy() -> Policy {
    Policy {
        local_only: LocalOnly::Off,
        floors: vec![],
    }
}

fn picked(name: &str) -> Result<AccountId, InferRefusal> {
    Ok(AccountId::parse(name).expect("id"))
}

struct Case {
    name: &'static str,
    class: DataClass,
    policy: Policy,
    candidates: Vec<RouteCandidate>,
    expected: Result<AccountId, InferRefusal>,
}

fn cases() -> Vec<Case> {
    let tier_other = RouteCandidate {
        tier: TierChoice::Other,
        ..cloud("cheap", 1)
    };
    let stopped = RouteCandidate {
        spend: SpendVerdict::Stop,
        ..cloud("capped", 1)
    };
    let asking = RouteCandidate {
        permission: Verdict::Ask,
        ..ollama()
    };
    let refused = RouteCandidate {
        permission: Verdict::Denied,
        ..ollama()
    };
    let peer_floor = Policy {
        local_only: LocalOnly::Off,
        floors: vec![ClassFloor {
            class: DataClass::Notes,
            floor: Floor::LocalNetwork,
        }],
    };
    vec![
        Case {
            name: "on device wins over cloud",
            class: DataClass::Public,
            policy: open_policy(),
            candidates: vec![cloud("a", 1), ollama()],
            expected: picked("ollama"),
        },
        Case {
            name: "local network wins over cloud",
            class: DataClass::Public,
            policy: open_policy(),
            candidates: vec![cloud("a", 1), peer()],
            expected: picked("peer"),
        },
        Case {
            name: "local-only leaves nothing but cloud: unavailable",
            class: DataClass::Public,
            policy: Policy::proposed(),
            candidates: vec![cloud("a", 1)],
            expected: Err(InferRefusal::Unavailable),
        },
        Case {
            name: "local-only mail runs on ollama",
            class: DataClass::Mail,
            policy: Policy::proposed(),
            candidates: vec![cloud("a", 1), ollama()],
            expected: picked("ollama"),
        },
        Case {
            name: "a floor with only cloud requires cloud",
            class: DataClass::Mail,
            policy: Policy {
                local_only: LocalOnly::Off,
                ..Policy::proposed()
            },
            candidates: vec![cloud("a", 1)],
            expected: Err(InferRefusal::RequiresCloud(DataClass::Mail)),
        },
        Case {
            name: "a local-network floor admits a peer",
            class: DataClass::Notes,
            policy: peer_floor.clone(),
            candidates: vec![cloud("a", 1), peer()],
            expected: picked("peer"),
        },
        Case {
            name: "a local-network floor refuses the cloud",
            class: DataClass::Notes,
            policy: peer_floor,
            candidates: vec![cloud("a", 1)],
            expected: Err(InferRefusal::RequiresCloud(DataClass::Notes)),
        },
        Case {
            name: "public goes to the cloud when local-only is off",
            class: DataClass::Public,
            policy: open_policy(),
            candidates: vec![cloud("a", 1)],
            expected: picked("a"),
        },
        Case {
            name: "the tier's choice beats a cheaper model",
            class: DataClass::Public,
            policy: open_policy(),
            candidates: vec![tier_other, cloud("dear", 9)],
            expected: picked("dear"),
        },
        Case {
            name: "cheaper wins among equals",
            class: DataClass::Public,
            policy: open_policy(),
            candidates: vec![cloud("dear", 9), cloud("cheap", 1)],
            expected: picked("cheap"),
        },
        Case {
            name: "a stopped cap is skipped",
            class: DataClass::Public,
            policy: open_policy(),
            candidates: vec![stopped.clone(), cloud("dear", 9)],
            expected: picked("dear"),
        },
        Case {
            name: "every cap stopped is over budget",
            class: DataClass::Public,
            policy: open_policy(),
            candidates: vec![stopped],
            expected: Err(InferRefusal::OverBudget),
        },
        Case {
            name: "an ungranted model needs a grant",
            class: DataClass::Public,
            policy: open_policy(),
            candidates: vec![asking],
            expected: Err(InferRefusal::NeedsGrant),
        },
        Case {
            name: "a refused model is denied",
            class: DataClass::Public,
            policy: open_policy(),
            candidates: vec![refused],
            expected: Err(InferRefusal::Denied),
        },
        Case {
            name: "nothing at all is unavailable",
            class: DataClass::Public,
            policy: open_policy(),
            candidates: vec![],
            expected: Err(InferRefusal::Unavailable),
        },
    ]
}

#[test]
fn route_prefers_this_computer_and_refuses_by_type() {
    for case in cases() {
        let got = route(
            RouteAsk { class: case.class },
            &case.candidates,
            &case.policy,
        )
        .map(|c| c.account);
        assert_eq!(got, case.expected, "{}", case.name);
    }
}

#[test]
fn floors_admit_by_closeness() {
    let cloud = Locality::Cloud { region: None };
    let cases = [
        (Floor::OnDevice, Locality::OnDevice, true),
        (Floor::OnDevice, Locality::LocalNetwork, false),
        (Floor::OnDevice, cloud.clone(), false),
        (Floor::LocalNetwork, Locality::LocalNetwork, true),
        (Floor::LocalNetwork, cloud.clone(), false),
        (Floor::Anywhere, cloud, true),
    ];
    for (floor, locality, expected) in cases {
        assert_eq!(floor.admits(&locality), expected, "{floor:?} {locality:?}");
    }
}

/// `route` as it was before `admit` was split out of it, kept as the reference the table below
/// holds the new one to.
fn legacy_route(
    ask: RouteAsk,
    candidates: &[RouteCandidate],
    policy: &Policy,
) -> Result<Chosen, InferRefusal> {
    let reachable: Vec<&RouteCandidate> = candidates
        .iter()
        .filter(|c| policy.local_only == LocalOnly::Off || !is_cloud(&c.locality))
        .collect();
    if reachable.is_empty() {
        return Err(InferRefusal::Unavailable);
    }
    let floor = policy.floor(ask.class);
    let admitted: Vec<&RouteCandidate> = reachable
        .into_iter()
        .filter(|c| floor.admits(&c.locality))
        .collect();
    if admitted.is_empty() {
        return Err(InferRefusal::RequiresCloud(ask.class));
    }
    let permitted: Vec<&RouteCandidate> = admitted
        .iter()
        .copied()
        .filter(|c| matches!(c.permission, Verdict::Granted { .. }))
        .collect();
    if permitted.is_empty() {
        let askable = admitted.iter().any(|c| c.permission == Verdict::Ask);
        return Err(if askable {
            InferRefusal::NeedsGrant
        } else {
            InferRefusal::Denied
        });
    }
    permitted
        .into_iter()
        .filter(|c| c.spend != SpendVerdict::Stop)
        .min_by_key(|c| (closeness(&c.locality), c.tier, price_rank(&c.billing)))
        .map(|c| Chosen {
            account: c.account.clone(),
            model: c.model.clone(),
            spend: c.spend,
        })
        .ok_or(InferRefusal::OverBudget)
}

/// Every combination of locality, tier choice, consent and spend verdict, each with its own
/// account name and a price that varies with its index.
fn pool() -> Vec<RouteCandidate> {
    let localities = [
        Locality::OnDevice,
        Locality::LocalNetwork,
        Locality::Cloud { region: None },
    ];
    let tiers = [TierChoice::Chosen, TierChoice::Other];
    let permissions = [granted(), Verdict::Ask, Verdict::Denied];
    let spends = [SpendVerdict::Within, SpendVerdict::Warn, SpendVerdict::Stop];
    let mut pool = Vec::new();
    for locality in &localities {
        for tier in tiers {
            for permission in &permissions {
                for spend in spends {
                    let n = pool.len();
                    let billing = match n % 3 {
                        0 => Billing::Free,
                        1 => Billing::PlanBudget,
                        _ => metered(u64::try_from(n % 7).expect("small")),
                    };
                    pool.push(RouteCandidate {
                        tier,
                        permission: permission.clone(),
                        spend,
                        ..candidate(&format!("a{n}"), locality.clone(), billing)
                    });
                }
            }
        }
    }
    pool
}

#[test]
fn route_decides_as_it_did_before_admit_was_split_out() {
    let pool = pool();
    let policies = [Policy::proposed(), open_policy(), {
        Policy {
            local_only: LocalOnly::Off,
            floors: vec![ClassFloor {
                class: DataClass::Notes,
                floor: Floor::LocalNetwork,
            }],
        }
    }];
    let classes = [DataClass::Public, DataClass::Mail, DataClass::Notes];
    let mut sets: Vec<Vec<RouteCandidate>> = vec![vec![]];
    for (i, a) in pool.iter().enumerate() {
        sets.push(vec![a.clone()]);
        for b in &pool[i + 1..] {
            sets.push(vec![a.clone(), b.clone()]);
            sets.push(vec![b.clone(), a.clone()]);
        }
    }
    let mut checked = 0;
    for set in &sets {
        for policy in &policies {
            for class in classes {
                let ask = RouteAsk { class };
                assert_eq!(
                    route(ask, set, policy),
                    legacy_route(ask, set, policy),
                    "{class:?} {policy:?} {:?}",
                    set.iter().map(|c| c.account.clone()).collect::<Vec<_>>()
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 10_000, "{checked}");
}

#[test]
fn admit_keeps_the_input_order_and_names_the_first_rule_that_leaves_none() {
    let pool = [cloud("a", 1), ollama(), peer()];
    let all = admit(
        RouteAsk {
            class: DataClass::Public,
        },
        &pool,
        &open_policy(),
    )
    .expect("all three");
    let names: Vec<_> = all.iter().map(|c| c.account.clone()).collect();
    assert_eq!(
        names,
        pool.iter().map(|c| c.account.clone()).collect::<Vec<_>>()
    );
    let only_local = admit(
        RouteAsk {
            class: DataClass::Public,
        },
        &pool,
        &Policy::proposed(),
    )
    .expect("local ones");
    assert_eq!(only_local.len(), 2);
}
