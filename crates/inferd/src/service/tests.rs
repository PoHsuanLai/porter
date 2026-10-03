use super::*;
use porter_core::DataClass;
use zbus::zvariant::{OwnedValue, Value};

fn options(pairs: &[(&str, &str)]) -> Details {
    pairs
        .iter()
        .map(|(key, value)| {
            (
                (*key).to_owned(),
                OwnedValue::try_from(Value::from(*value)).expect("value"),
            )
        })
        .collect()
}

const TRACE: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

#[test]
fn a_traceparent_in_the_options_is_read_when_valid_and_ignored_otherwise() {
    assert_eq!(
        trace_of(&options(&[("traceparent", TRACE)])).map(|t| t.as_str().to_owned()),
        Some(TRACE.to_owned())
    );
    assert!(trace_of(&options(&[("traceparent", "garbage")])).is_none());
    assert!(
        trace_of(&options(&[("unknown", TRACE)])).is_none(),
        "an unknown key is ignored"
    );
    assert!(trace_of(&Details::new()).is_none());
}

#[test]
fn refusals_and_closed_sets_are_written_as_their_slugs() {
    let refusals = [
        (
            InferRefusal::RequiresCloud(DataClass::Prompt),
            "requires_cloud",
        ),
        (InferRefusal::Unavailable, "unavailable"),
        (InferRefusal::NeedsGrant, "needs_grant"),
        (InferRefusal::Denied, "denied"),
        (InferRefusal::OverBudget, "over_budget"),
        (InferRefusal::Unsupported, "unsupported"),
    ];
    for (refusal, slug_text) in refusals {
        assert_eq!(refusal_slug(&refusal), slug_text);
    }
    assert_eq!(
        slug(
            &porter_core::consent::Availability::AvailableNeedsConsent,
            "kind"
        ),
        "available_needs_consent"
    );
    assert_eq!(slug(&DataClass::Prompt, "kind"), "prompt");
}

#[test]
fn class_and_tier_slugs_parse_and_anything_else_is_an_invalid_argument() {
    assert_eq!(
        parse_slug::<DataClass>("prompt").ok(),
        Some(DataClass::Prompt)
    );
    assert_eq!(parse_slug::<Tier>("best").ok(), Some(Tier::Best));
    for bad in ["", "Prompt", "gossip"] {
        assert!(
            matches!(
                parse_slug::<DataClass>(bad),
                Err(fdo::Error::InvalidArgs(_))
            ),
            "{bad:?}"
        );
    }
}
