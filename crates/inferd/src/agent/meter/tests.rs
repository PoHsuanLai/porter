use super::*;
use serde_json::json;

fn stream(shape: Shape, pieces: &[&str]) -> Seen {
    let mut reading = Reading::of_stream(shape);
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
    let mut anthropic = Reading::of_body(Shape::Anthropic);
    anthropic.finish_body(
        json!({"usage": {"input_tokens": 3, "output_tokens": 2}})
            .to_string()
            .as_bytes(),
    );
    assert_eq!(anthropic.seen().usage().map(|u| u.output), Some(Tokens(2)));
    let mut openai = Reading::of_body(Shape::OpenAi);
    openai.finish_body(
        json!({"usage": {"prompt_tokens": 3, "completion_tokens": 2}})
            .to_string()
            .as_bytes(),
    );
    assert_eq!(openai.seen().usage().map(|u| u.input), Some(Tokens(3)));
    let mut none = Reading::of_body(Shape::OpenAi);
    none.finish_body(b"{\"choices\":[]}");
    assert_eq!(none.seen().usage(), None);
    assert_eq!(
        assumed().input,
        Tokens(1_000),
        "a missing figure is never free"
    );
}
