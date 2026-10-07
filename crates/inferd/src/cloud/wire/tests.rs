use super::*;
use serde_json::json;

fn provider(id: &str) -> ProviderId {
    ProviderId(id.to_owned())
}

#[test]
fn every_provider_this_build_speaks_has_a_door_and_the_others_have_none() {
    let doors = Doors::real();
    let cases = [
        ("openrouter", Some(("openrouter.ai", "/api/v1"))),
        ("openai", Some(("api.openai.com", "/v1"))),
        ("moonshot", Some(("api.moonshot.ai", "/v1"))),
        (
            "google-ai",
            Some(("generativelanguage.googleapis.com", "/v1beta/openai")),
        ),
        // Anthropic's Messages API is not a wire a turn speaks; only an agent's request is
        // forwarded there (`Cloud::agent_client`).
        ("anthropic", Some(("api.anthropic.com", "/v1"))),
        ("nobody", None),
    ];
    for (id, want) in cases {
        let got = doors
            .of(&provider(id))
            .map(|door| (door.host.as_str(), door.base.as_str()));
        assert_eq!(got, want, "{id}");
        if let Some(door) = doors.of(&provider(id)) {
            assert_eq!(door.port, 443, "{id}");
        }
    }
}

#[test]
fn a_test_can_move_one_provider_to_loopback() {
    let door = Door {
        host: "localhost".into(),
        port: 4443,
        base: "/api/v1".into(),
    };
    let doors = Doors::real().with("openrouter", door.clone());
    assert_eq!(doors.of(&provider("openrouter")), Some(&door));
    assert_eq!(
        doors.of(&provider("openai")).map(|d| d.host.as_str()),
        Some("api.openai.com")
    );
}

#[test]
fn the_flavor_is_openrouters_for_the_gateway_and_the_standard_one_for_companies() {
    let cases = [
        ("openrouter", Flavor::OpenRouter),
        ("openai", Flavor::LiteLlm),
        ("moonshot", Flavor::LiteLlm),
        ("google-ai", Flavor::LiteLlm),
    ];
    for (id, want) in cases {
        assert_eq!(flavor_of(&provider(id)), want, "{id}");
    }
}

fn body(tools: bool) -> String {
    let mut body = json!({"model": "m", "temperature": 0.7, "messages": []});
    if tools {
        body["tools"] = json!([{"type": "function"}]);
    }
    body.to_string()
}

fn applied(shape: BodyShape, text: &str) -> Value {
    serde_json::from_str(&shape.apply(text)).expect("json")
}

#[test]
fn temperature_stays_when_the_app_chose_a_sampling_and_goes_when_it_did_not() {
    let sent = BodyShape::of(&provider("openai"), "gpt-6-astra", Temperature::Sent);
    assert_eq!(applied(sent, &body(false))["temperature"], 0.7);
    let default = BodyShape::of(
        &provider("openai"),
        "gpt-6-astra",
        Temperature::ProviderDefault,
    );
    assert!(applied(default, &body(false)).get("temperature").is_none());
}

#[test]
fn luna_takes_tools_only_with_reasoning_off_in_each_provider_spelling() {
    let cases = [
        ("openai", true, Some(("reasoning_effort", json!("none")))),
        ("openai", false, None),
        (
            "openrouter",
            true,
            Some(("reasoning", json!({"effort": "none"}))),
        ),
        ("openrouter", false, None),
    ];
    for (id, tools, want) in cases {
        let shape = BodyShape::of(&provider(id), "gpt-6-luna", Temperature::Sent);
        let got = applied(shape, &body(tools));
        match want {
            Some((field, value)) => assert_eq!(got[field], value, "{id} tools={tools}"),
            None => {
                assert!(got.get("reasoning_effort").is_none(), "{id}");
                assert!(got.get("reasoning").is_none(), "{id}");
            }
        }
    }
}

#[test]
fn no_other_model_has_its_reasoning_touched() {
    for model in [
        "gpt-6-astra",
        "claude-opus-5.5",
        "kimi-k3",
        "gemini-3.1-pro",
    ] {
        let shape = BodyShape::of(&provider("openrouter"), model, Temperature::Sent);
        assert_eq!(shape.no_reasoning, NoReasoning::Untouched, "{model}");
        let got = applied(shape, &body(true));
        assert!(got.get("reasoning").is_none() && got.get("reasoning_effort").is_none());
    }
}

#[test]
fn a_body_that_is_not_an_object_is_left_alone() {
    let shape = BodyShape::of(
        &provider("openai"),
        "gpt-6-luna",
        Temperature::ProviderDefault,
    );
    assert_eq!(shape.apply("[1,2]"), "[1,2]");
    assert_eq!(shape.apply("not json"), "not json");
}
