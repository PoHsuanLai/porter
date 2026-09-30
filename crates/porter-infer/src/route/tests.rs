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
