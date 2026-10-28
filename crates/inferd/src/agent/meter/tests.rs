use super::*;
use serde_json::json;

fn stream(shape: Shape, pieces: &[&str]) -> Seen {
    stream_from(shape, Charges::NotReported, pieces)
}

fn stream_from(shape: Shape, charges: Charges, pieces: &[&str]) -> Seen {
    let mut reading = Reading::of_stream(shape, charges);
    for piece in pieces {
        reading.feed(piece.as_bytes());
    }
    reading.seen()
}

#[test]
fn an_anthropic_stream_gives_input_from_the_start_and_output_from_the_end() {
    let start = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":25,\"cache_read_input_tokens\":10,\"cache_creation_input_tokens\":5,\"output_tokens\":1}}}\n\n";
    let delta = "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":42}}\n\n";
    // Split mid-event: the chunking must not matter.
    let (a, b) = start.split_at(60);
    let usage = stream(Shape::Anthropic, &[a, b, delta])
        .usage()
        .expect("usage");
    assert_eq!(
        (usage.input, usage.output, usage.cached),
        (Tokens(40), Tokens(42), Tokens(10))
    );
}

#[test]
fn a_later_event_repeating_input_without_the_cache_figures_does_not_undo_the_total() {
    let start =
        "data: {\"message\":{\"usage\":{\"input_tokens\":25,\"cache_read_input_tokens\":10}}}\n\n";
    let delta = "data: {\"usage\":{\"input_tokens\":25,\"output_tokens\":3}}\n\n";
    let usage = stream(Shape::Anthropic, &[start, delta])
        .usage()
        .expect("usage");
    assert_eq!((usage.input, usage.output), (Tokens(35), Tokens(3)));
}

#[test]
fn an_openai_stream_gives_usage_from_its_final_chunk() {
    let mid = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}],\"usage\":null}\n\n";
    let last = "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4,\"prompt_tokens_details\":{\"cached_tokens\":3}}}\n\ndata: [DONE]\n\n";
    let usage = stream(Shape::OpenAi, &[mid, last]).usage().expect("usage");
    assert_eq!(
        (usage.input, usage.output, usage.cached),
        (Tokens(9), Tokens(4), Tokens(3))
    );
}

#[test]
fn a_whole_body_is_read_in_either_shape_and_a_body_with_no_figures_gives_none() {
    let mut anthropic = Reading::of_body(Shape::Anthropic, Charges::NotReported);
    anthropic.finish_body(
        json!({"usage": {"input_tokens": 3, "output_tokens": 2}})
            .to_string()
            .as_bytes(),
    );
    assert_eq!(anthropic.seen().usage().map(|u| u.output), Some(Tokens(2)));
    let mut openai = Reading::of_body(Shape::OpenAi, Charges::NotReported);
    openai.finish_body(
        json!({"usage": {"prompt_tokens": 3, "completion_tokens": 2}})
            .to_string()
            .as_bytes(),
    );
    assert_eq!(openai.seen().usage().map(|u| u.input), Some(Tokens(3)));
    let mut none = Reading::of_body(Shape::OpenAi, Charges::NotReported);
    none.finish_body(b"{\"choices\":[]}");
    assert_eq!(none.seen().usage(), None);
}

/// A reply that came whole with figures, or part way, is counted with what the provider reported;
/// one with no figure is a turn of no tokens; a request no model ran is not a turn at all.
#[test]
fn a_request_is_counted_as_reported_as_an_empty_turn_when_unknown_and_not_when_no_model_ran() {
    let mut reported = Reading::of_body(Shape::OpenAi, Charges::NotReported);
    reported.finish_body(br#"{"usage":{"prompt_tokens":3,"completion_tokens":2}}"#);
    let reported = reported.seen();
    let silent = Seen::default();
    assert_eq!(
        counted(End::Answered, reported).map(|used| (used.input, used.output)),
        Some((Tokens(3), Tokens(2)))
    );
    let empty = counted(End::Answered, silent).expect("a turn");
    assert_eq!(
        (empty.input, empty.output, empty.charge),
        (Tokens(0), Tokens(0), None)
    );
    assert_eq!(counted(End::NotRun, reported), None);
    assert_eq!(
        counted(End::Unknown, reported).map(|used| used.input),
        Some(Tokens(3)),
        "what was seen of a cut-off reply is counted"
    );
}

const COST_CHUNK: &str = "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4,\"cost\":0.00095}}\n\ndata: [DONE]\n\n";

/// A charge is read only from a provider that says it reports one, and exactly as written.
#[test]
fn a_charge_is_read_from_a_provider_that_reports_one_and_from_no_other() {
    let from = |charges| stream_from(Shape::OpenAi, charges, &[COST_CHUNK]).usage();
    let charged = from(Charges::PerReply).expect("usage");
    assert_eq!(charged.charge, Some(Money::usd(Decimal::new(95, 5))));
    assert_eq!((charged.input, charged.output), (Tokens(9), Tokens(4)));
    assert_eq!(from(Charges::NotReported).expect("usage").charge, None);
}

#[test]
fn a_charge_that_cannot_be_read_is_no_charge() {
    for body in [
        r#"{"usage":{"prompt_tokens":1,"cost":-0.5}}"#,
        r#"{"usage":{"prompt_tokens":1,"cost":"free"}}"#,
        r#"{"usage":{"prompt_tokens":1,"cost":null}}"#,
        r#"{"usage":{"prompt_tokens":1}}"#,
        r#"{"usage":null}"#,
    ] {
        let mut reading = Reading::of_body(Shape::OpenAi, Charges::PerReply);
        reading.finish_body(body.as_bytes());
        assert_eq!(
            reading.seen().usage().and_then(|u| u.charge),
            None,
            "{body}"
        );
    }
    let mut anthropic = Reading::of_body(Shape::Anthropic, Charges::PerReply);
    anthropic.finish_body(br#"{"usage":{"input_tokens":1,"cost":0.5}}"#);
    assert_eq!(
        anthropic.seen().usage().and_then(|u| u.charge),
        None,
        "only the OpenAI shape carries a cost"
    );
}
