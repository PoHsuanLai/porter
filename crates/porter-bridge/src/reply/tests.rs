use super::*;
use porter_core::{AccountId, Locality, ModelId};

fn served() -> pi::ServedBy {
    pi::ServedBy {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse("m").expect("id"),
        locality: Locality::OnDevice,
    }
}

fn call(name: &str) -> sp::ToolCall {
    sp::ToolCall {
        id: sp::ToolCallId("c1".into()),
        name: sp::ToolName::new(name).expect("name"),
        input: sp::JsonText::new(r#"{"a":1}"#).expect("json"),
    }
}

fn end(stop: sp::StopReason) -> sp::TurnEnd {
    sp::TurnEnd {
        stop,
        usage: sp::TurnUsage {
            input: sp::Tokens(10),
            output: sp::Tokens(4),
            cached: sp::Tokens(3),
            images: sp::ImageCount(1),
        },
        served: sp::ModelName("m".into()),
        first_token: None,
    }
}

#[test]
fn only_what_the_client_sees_becomes_an_event() {
    let cases: Vec<(sp::TurnEvent, Option<pi::InferEvent>)> = vec![
        (
            sp::TurnEvent::TextDelta("hi".into()),
            Some(pi::InferEvent::TextDelta("hi".into())),
        ),
        (
            sp::TurnEvent::ThoughtDelta("hm".into()),
            Some(pi::InferEvent::ThoughtDelta("hm".into())),
        ),
        (
            sp::TurnEvent::ToolCallDone(call("lookup")),
            Some(pi::InferEvent::ToolCall(pi::ToolCallPart {
                id: pi::ToolCallId("c1".into()),
                name: pi::ToolName::parse("lookup").expect("name"),
                args: pi::JsonText::parse(r#"{"a":1}"#).expect("json"),
            })),
        ),
        (sp::TurnEvent::ThoughtSealed(sp::ThoughtSeal::None), None),
        (
            sp::TurnEvent::ToolCallStarted {
                index: sp::CallIndex(0),
                id: sp::ToolCallId("c1".into()),
                name: sp::ToolName::new("lookup").expect("name"),
            },
            None,
        ),
        (
            sp::TurnEvent::ToolCallDelta {
                index: sp::CallIndex(0),
                fragment: "{".into(),
            },
            None,
        ),
        (
            sp::TurnEvent::Safety(sp::SafetySignal::Blocked("x".into())),
            None,
        ),
        (sp::TurnEvent::Usage(sp::TurnUsage::default()), None),
    ];
    for (given, expected) in cases {
        assert_eq!(event(&given), expected, "{given:?}");
    }
}

#[test]
fn a_reply_is_gathered_from_the_events_in_order() {
    let mut gathered = Gathered::default();
    for piece in [
        sp::TurnEvent::ThoughtDelta("let me ".into()),
        sp::TurnEvent::ThoughtDelta("think".into()),
        sp::TurnEvent::TextDelta("Hel".into()),
        sp::TurnEvent::TextDelta("lo".into()),
        sp::TurnEvent::ToolCallDone(call("lookup")),
        sp::TurnEvent::Usage(sp::TurnUsage::default()),
    ] {
        gathered.take(&piece);
    }
    let reply = gathered.chat_reply(&end(sp::StopReason::ToolUse), served());
    assert_eq!(reply.text, "Hello");
    assert_eq!(reply.thought.as_deref(), Some("let me think"));
    assert_eq!(reply.tool_calls.len(), 1);
    assert_eq!(reply.stop, pi::StopReason::ToolUse);
    assert_eq!(
        reply.usage,
        pi::TokenUsage {
            input: porter_core::Tokens(10),
            output: porter_core::Tokens(4),
            cached: porter_core::Tokens(3),
        }
    );
    assert_eq!(reply.served, served());
    // No thought, no `thought`.
    assert_eq!(
        Gathered::default()
            .chat_reply(&end(sp::StopReason::EndTurn), served())
            .thought,
        None
    );
}

#[test]
fn every_stop_reason_maps_both_ways() {
    for stop_reason in [
        sp::StopReason::EndTurn,
        sp::StopReason::ToolUse,
        sp::StopReason::MaxTokens,
        sp::StopReason::StopSequence,
        sp::StopReason::ContentFilter,
    ] {
        assert_eq!(stop_back(stop(stop_reason)), stop_reason);
    }
}

#[test]
fn every_provider_error_is_a_model_error_the_app_can_act_on() {
    use pi::ModelError as M;
    use sp::ProviderError as P;
    let cases = [
        (P::Unreachable, M::Unreachable),
        (P::Timeout, M::Unreachable),
        (P::Server(sp::ServerStatus(502)), M::Unreachable),
        (P::NotReady, M::NotReady),
        (P::RateLimited(sp::RetrySeconds(7)), M::RateLimited(7)),
        (P::Unauthorized, M::Unauthorized),
        (
            P::ContextOverflow {
                limit: sp::Tokens(10),
            },
            M::ContextOverflow,
        ),
        (P::BadRequest("x".into()), M::Refused),
        (P::Refused("x".into()), M::Refused),
        (P::Unreadable("x".into()), M::Unreadable),
    ];
    for (given, expected) in cases {
        assert_eq!(model_error(&given), expected, "{given:?}");
    }
}

#[test]
fn vectors_keep_their_numbers_and_widths_are_counted() {
    let mapped = vectors(vec![
        sp::EmbedVector(vec![0.5, 0.25]),
        sp::EmbedVector(vec![1.0, 2.0]),
    ]);
    assert_eq!(mapped[0], pi::EmbedVector(vec![0.5, 0.25]));
    assert_eq!(width(&mapped[1]), Dims(2));
}
