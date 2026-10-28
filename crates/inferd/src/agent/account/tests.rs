use super::*;
use model_http::{HttpStatus, RequestId};

fn head(status: u16, retry: Option<u32>) -> ResponseHead {
    ResponseHead {
        status: HttpStatus(status),
        body: model_http::BodyKind::Json,
        retry_after: retry.map(model_http::WaitSeconds),
        request_id: RequestId::new("req").ok(),
    }
}

#[test]
fn a_providers_failure_is_told_as_a_kind_and_never_with_its_body() {
    let cases = [
        (429, Some(7), Cause::RateLimited(Some(7))),
        (429, None, Cause::RateLimited(None)),
        (529, None, Cause::Overloaded),
        (503, None, Cause::Overloaded),
        (401, None, Cause::Upstream),
        (403, None, Cause::Upstream),
        (404, None, Cause::NotFound),
        (400, None, Cause::Invalid),
        (422, None, Cause::Invalid),
        (500, None, Cause::Upstream),
    ];
    for (status, retry, want) in cases {
        let failure = upstream(Some(&head(status, retry)));
        assert_eq!(failure.cause, want, "{status}");
    }
    assert_eq!(upstream(None).cause, Cause::Upstream);
}

#[test]
fn an_openai_stream_asks_for_usage_and_every_other_body_goes_as_it_came() {
    let stream = Request::new(
        "POST",
        "/v1/chat/completions",
        &[],
        br#"{"model":"m","stream":true,"messages":[]}"#,
    );
    let sent = body_for(&stream, Shape::OpenAi, true).expect("body");
    let value: Value = serde_json::from_str(&sent).expect("json");
    assert_eq!(value["stream_options"]["include_usage"], true);
    assert_eq!(value["model"], "m");
    let whole = br#"{ "model": "m",  "tools": [{"type":"web_search_20250305"}] }"#;
    let request = Request::new("POST", "/v1/messages", &[], whole);
    assert_eq!(
        body_for(&request, Shape::Anthropic, true)
            .expect("body")
            .as_bytes(),
        whole,
        "nothing of an Anthropic request is touched"
    );
    assert!(
        body_for(
            &Request::new("POST", "/", &[], &[0xff, 0xfe]),
            Shape::Anthropic,
            false
        )
        .is_err()
    );
}

#[test]
fn anthropic_requests_carry_the_version_and_betas_the_agent_asked_for() {
    let with = Request::new(
        "POST",
        "/v1/messages",
        &[
            ("anthropic-version", "2024-01-01"),
            ("anthropic-beta", "tools-2024"),
        ],
        b"{}",
    );
    assert_eq!(
        anthropic_headers(&with),
        [
            ("anthropic-version".to_owned(), "2024-01-01".to_owned()),
            ("anthropic-beta".to_owned(), "tools-2024".to_owned())
        ]
    );
    let without = Request::new("POST", "/v1/messages", &[], b"{}");
    assert_eq!(
        anthropic_headers(&without),
        [("anthropic-version".to_owned(), "2023-06-01".to_owned())]
    );
}
