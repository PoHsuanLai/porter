use super::*;
use crate::replay::cassette::Call;

fn text(pieces: &[Vec<u8>]) -> String {
    String::from_utf8_lossy(&pieces.concat()).into_owned()
}

#[test]
fn delivery_follows_the_stream_flag() {
    assert_eq!(Delivery::of(&json!({"stream": true})), Delivery::Stream);
    assert_eq!(Delivery::of(&json!({"stream": false})), Delivery::Whole);
    assert_eq!(Delivery::of(&json!({})), Delivery::Whole);
}

#[test]
fn a_streamed_reply_is_chunked_events_ending_in_done() {
    let out = text(&reply(&Reply::Text("hello".into()), Delivery::Stream));
    assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
    assert!(out.contains("\"content\":\"hello\""), "{out}");
    assert!(out.contains("\"finish_reason\":\"stop\""), "{out}");
    assert!(
        out.contains("data: [DONE]") && out.ends_with("0\r\n\r\n"),
        "{out}"
    );
    let calls = vec![Call {
        name: "f".into(),
        arguments: json!({"a": 1}),
    }];
    let out = text(&reply(&Reply::Calls(calls.clone()), Delivery::Stream));
    assert!(
        out.contains("\"name\":\"f\"") && out.contains("{\\\"a\\\":1}"),
        "{out}"
    );
    assert!(out.contains("\"finish_reason\":\"tool_calls\""), "{out}");
    let out = text(&reply(&Reply::Calls(calls), Delivery::Whole));
    assert!(
        out.contains("Content-Length") && out.contains("\"tool_calls\""),
        "{out}"
    );
}

#[test]
fn failures_and_misses_carry_a_type() {
    let out = text(&reply(&Reply::Fail(503), Delivery::Stream));
    assert!(out.starts_with("HTTP/1.1 503"), "{out}");
    let out = text(&miss(&ReplayError::NoEntry {
        entries: 1,
        answered: 1,
    }));
    assert!(
        out.starts_with("HTTP/1.1 422") && out.contains("replay_miss"),
        "{out}"
    );
    assert!(text(&not_found()).starts_with("HTTP/1.1 404"));
    assert!(text(&health()).contains("\"ok\""));
}

#[test]
fn a_recorded_cut_stream_has_no_terminator() {
    use model_http::{BodyKind, HttpStatus};
    use model_replay::{HeadPrint, WireFrame};
    let wire = |end| WireReply {
        head: HeadPrint {
            status: HttpStatus(200),
            body: BodyKind::EventStream,
            retry_after: None,
        },
        body: WireBody::Frames(vec![WireFrame {
            event: None,
            data: "{}".into(),
        }]),
        end,
    };
    assert!(
        text(&reply(
            &Reply::Wire(wire(WireEnd::Complete)),
            Delivery::Stream
        ))
        .ends_with("0\r\n\r\n")
    );
    assert!(
        !text(&reply(&Reply::Wire(wire(WireEnd::Cut)), Delivery::Stream)).ends_with("0\r\n\r\n")
    );
}
