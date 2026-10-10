use super::*;
use porter_core::capability::{Capability, LlmCap, LlmFeature, LlmWire};
use porter_core::consent::{GrantScope, Verdict};
use porter_core::need::{DimsNeed, EmbedNeed, LlmNeed};
use porter_core::{AccountId, Billing, GrantId, Locality, Tokens};
use porter_infer::{
    AutoPolicy, LicenceClass, LocalOnly, ModelCard, PlaceKind, Policy, Readiness, SwapCost,
};

fn llm_cap() -> Capability {
    Capability::Llm(LlmCap {
        features: [LlmFeature::Chat, LlmFeature::Tools].into(),
        context: Tokens(8192),
        max_output: Tokens(1024),
        wire: LlmWire::ChatCompletions,
    })
}

fn need() -> Need {
    Need::Llm(LlmNeed::new([LlmFeature::Chat].into(), Tokens(1000)))
}

fn embeddings() -> Need {
    Need::Embeddings(EmbedNeed::new(
        DimsNeed::Any,
        [porter_core::capability::Modality::Text].into(),
    ))
}

fn place(text: &str) -> PlaceId {
    PlaceId::parse(text).expect("place")
}

fn granted() -> Verdict {
    Verdict::Granted {
        grant: GrantId::parse("g").expect("id"),
        scope: GrantScope::Always,
    }
}

/// A model at `place_text`, with the locality that kind of place has and consent given.
fn at(place_text: &str, model: &str, serving: Serving) -> Placed {
    let id = place(place_text);
    let (account, locality) = match id.kind() {
        PlaceKind::ThisComputer => ("local".to_owned(), Locality::OnDevice),
        PlaceKind::OwnComputer => ("local".to_owned(), Locality::LocalNetwork),
        PlaceKind::CloudAccount => (
            place_text.trim_start_matches("account:").to_owned(),
            Locality::Cloud { region: None },
        ),
    };
    let card = ModelCard {
        account: AccountId::parse(&account).expect("id"),
        model: ModelId::parse(model).expect("id"),
        locality,
        billing: Billing::Free,
        capabilities: vec![llm_cap()],
    };
    let mut listed = Listed::new(
        card,
        Readiness::Ready,
        SwapCost::Resident,
        LicenceClass::Open,
    );
    listed.permission = granted();
    Placed {
        listed,
        place: id,
        serving,
    }
}

/// Local-only off, the shipped floors.
fn open() -> Policy {
    Policy {
        local_only: LocalOnly::Off,
        ..Policy::proposed()
    }
}

type Got = Result<(String, PlaceId), Unplaced>;

fn run_need(
    need: &Need,
    candidates: &[Placed],
    allowed: &Allowed,
    class: DataClass,
    policy: &Policy,
) -> Got {
    let tiers = TierMap::default();
    choose_in(
        need,
        class,
        Tier::Balanced,
        candidates,
        allowed,
        Rules {
            policy,
            tiers: &tiers,
            auto: AutoPolicy::default(),
        },
    )
    .map(|(decided, place)| (decided.chosen.model.as_str().to_owned(), place))
}

fn run(candidates: &[Placed], allowed: &Allowed, class: DataClass, policy: &Policy) -> Got {
    run_need(&need(), candidates, allowed, class, policy)
}

fn allowed(places: &[&str]) -> Allowed {
    Allowed::new(
        places.iter().map(|text| place(text)).collect(),
        BTreeMap::new(),
    )
}

fn refusal(got: Got) -> PlaceRefusal {
    match got {
        Err(Unplaced::NoPlace(refusal)) => refusal,
        other => panic!("a refusal naming a reason, got {other:?}"),
    }
}

fn refused(reason: NoPlaceReason, would_need: Option<PlaceKind>) -> PlaceRefusal {
    PlaceRefusal::new(reason, would_need)
}

#[test]
fn the_callers_order_decides_not_the_usual_nearest_first() {
    let candidates = [
        at("this-computer", "near", Serving::Now),
        at("account:work", "far", Serving::Now),
    ];
    let both_ways = [
        (
            &["this-computer", "account:work"][..],
            "near",
            "this-computer",
        ),
        (
            &["account:work", "this-computer"][..],
            "far",
            "account:work",
        ),
    ];
    for (order, model, where_) in both_ways {
        let got = run(&candidates, &allowed(order), DataClass::Public, &open());
        assert_eq!(got, Ok((model.to_owned(), place(where_))), "{order:?}");
    }
}

#[test]
fn a_place_that_cannot_serve_hands_over_to_the_next_of_the_set() {
    let candidates = [
        at("this-computer", "near", Serving::NotNow),
        at("computer:lab", "lab-model", Serving::Now),
    ];
    let got = run(
        &candidates,
        &allowed(&["this-computer", "computer:lab"]),
        DataClass::Public,
        &open(),
    );
    assert_eq!(got, Ok(("lab-model".to_owned(), place("computer:lab"))));
}

#[test]
fn a_cloud_account_outside_the_set_is_never_used_even_when_it_is_the_only_one_able() {
    let candidates = [
        at("this-computer", "near", Serving::NotNow),
        at("account:work", "far", Serving::Now),
    ];
    let got = run(
        &candidates,
        &allowed(&["this-computer"]),
        DataClass::Public,
        &open(),
    );
    assert_eq!(
        refusal(got),
        refused(NoPlaceReason::NotReady, Some(PlaceKind::CloudAccount))
    );
}

#[test]
fn each_reason_is_named_with_the_kind_that_would_have_served() {
    // NotReady: a place of the set could, but is not ready; nothing outside could.
    let down = [at("this-computer", "near", Serving::NotNow)];
    let got = run(
        &down,
        &allowed(&["this-computer"]),
        DataClass::Public,
        &open(),
    );
    assert_eq!(refusal(got), refused(NoPlaceReason::NotReady, None));

    // FloorRefused: the prompt class may not leave this computer and the only allowed place is a
    // cloud account; this computer, outside the set, could.
    let both = [
        at("this-computer", "near", Serving::Now),
        at("account:work", "far", Serving::Now),
    ];
    let got = run(
        &both,
        &allowed(&["account:work"]),
        DataClass::Prompt,
        &open(),
    );
    assert_eq!(
        refusal(got),
        refused(NoPlaceReason::FloorRefused, Some(PlaceKind::ThisComputer))
    );
    // The local-only switch is the same reason.
    let got = run(
        &both,
        &allowed(&["account:work"]),
        DataClass::Public,
        &Policy::proposed(),
    );
    assert_eq!(
        refusal(got),
        refused(NoPlaceReason::FloorRefused, Some(PlaceKind::ThisComputer))
    );
    // ... and a floor that also keeps the outside place from serving gives no `would_need`.
    let cloud_only = [at("account:work", "far", Serving::Now)];
    let got = run(
        &cloud_only,
        &allowed(&["account:work"]),
        DataClass::Prompt,
        &open(),
    );
    assert_eq!(refusal(got), refused(NoPlaceReason::FloorRefused, None));

    // ModelNotOffered: the model pinned for the place is not there.
    let here = [at("this-computer", "near", Serving::Now)];
    let pinned = Allowed::new(
        vec![place("this-computer")],
        BTreeMap::from([(place("this-computer"), ModelId::parse("other").expect("id"))]),
    );
    let got = run(&here, &pinned, DataClass::Public, &open());
    assert_eq!(refusal(got), refused(NoPlaceReason::ModelNotOffered, None));
    // With another place outside the set able to serve, the refusal says what it would take.
    let elsewhere = [
        at("this-computer", "near", Serving::Now),
        at("computer:lab", "lab-model", Serving::Now),
    ];
    let got = run(&elsewhere, &pinned, DataClass::Public, &open());
    assert_eq!(
        refusal(got),
        refused(NoPlaceReason::ModelNotOffered, Some(PlaceKind::OwnComputer))
    );

    // NoneCapable: no place of the set does this kind of work at all.
    let got = run_need(
        &embeddings(),
        &here,
        &allowed(&["this-computer"]),
        DataClass::Public,
        &open(),
    );
    assert_eq!(refusal(got), refused(NoPlaceReason::NoneCapable, None));
    // A place nothing is known of, and an empty set, are NoneCapable too.
    for set in [allowed(&["computer:gone"]), allowed(&[])] {
        let got = run(&[], &set, DataClass::Public, &open());
        assert_eq!(refusal(got), refused(NoPlaceReason::NoneCapable, None));
    }
}

#[test]
fn when_several_reasons_hold_the_first_of_the_order_wins() {
    let candidates = [
        at("this-computer", "near", Serving::NotNow),
        at("account:work", "far", Serving::Now),
    ];
    // This computer is not ready; the account is refused by the floor: not ready is named.
    let got = run(
        &candidates,
        &allowed(&["this-computer", "account:work"]),
        DataClass::Prompt,
        &open(),
    );
    assert_eq!(refusal(got).reason, NoPlaceReason::NotReady);
}

#[test]
fn a_pin_is_the_callers_word_for_that_place() {
    let candidates = [
        at("this-computer", "a", Serving::Now),
        at("this-computer", "b", Serving::Now),
    ];
    let pinned = Allowed::new(
        vec![place("this-computer")],
        BTreeMap::from([(place("this-computer"), ModelId::parse("b").expect("id"))]),
    );
    assert_eq!(
        run(&candidates, &pinned, DataClass::Public, &open()),
        Ok(("b".to_owned(), place("this-computer")))
    );
}

#[test]
fn a_rule_other_than_the_place_keeps_its_own_refusal() {
    let mut asks = at("account:work", "far", Serving::Now);
    asks.listed.permission = Verdict::Ask;
    let got = run(
        &[asks],
        &allowed(&["account:work"]),
        DataClass::Public,
        &open(),
    );
    assert_eq!(got, Err(Unplaced::Other(InferRefusal::NeedsGrant.into())));
}

#[test]
fn a_place_named_twice_stays_where_it_first_was() {
    let set = allowed(&["computer:a", "this-computer", "computer:a"]);
    assert_eq!(
        set.places(),
        [place("computer:a"), place("this-computer")].as_slice()
    );
    assert!(set.contains(&place("computer:a")));
    assert!(!set.contains(&place("account:x")));
}
