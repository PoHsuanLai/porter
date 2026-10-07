//! inferd as an external coding agent's model endpoint over a model on this computer (a supervised
//! engine, an attached one), end to end on a private bus: the translation of Anthropic and OpenAI
//! requests to the engine's chat and back, with no key and no spend.

mod hosting;

use hosting::agent::*;
use hosting::engine::Chat;
use hosting::entries;
use hosting::rig::{AgentsPlan, Plan, World};
use inferd::peers::Role;
use inferd::settings::AgentEndpoint;
use porter_infer::Policy;
use serde_json::json;

// ---- a model on this computer --------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn a_local_route_serves_an_anthropic_tool_conversation_with_no_key_and_no_spend() {
    let call = Chat::Call {
        name: "read_file",
        arguments: r#"{"path":"lib.rs"}"#.into(),
    };
    let world = World::start(plan(vec![anthropic_account()], say("unused"), call)).await;
    let endpoint = open(
        &world,
        &model_route("tiny-chat", "listed", &[]),
        "anthropic_messages",
    )
    .await;
    let sent = anthropic_body("tiny-chat", false);
    let reply = messages(&endpoint, &sent).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let message = reply.json();
    assert_eq!(message["stop_reason"], "tool_use");
    assert_eq!(message["content"][0]["type"], "tool_use");
    assert_eq!(message["content"][0]["name"], "read_file");
    assert_eq!(message["content"][0]["input"]["path"], "lib.rs");
    assert_eq!(message["model"], "tiny-chat");
    assert_eq!(message["usage"]["input_tokens"], 5);

    // The engine saw the OpenAI shape: the tool declared, the call and its result as tool
    // messages, the system prompt first.
    let engine = world.engines.get("tiny-chat").expect("engine");
    let bodies = engine.bodies("/v1/chat/completions");
    assert_eq!(bodies.len(), 1);
    let messages_seen = bodies[0]["messages"].as_array().expect("messages");
    let roles: Vec<&str> = messages_seen
        .iter()
        .map(|m| m["role"].as_str().expect("role"))
        .collect();
    assert_eq!(roles, ["system", "user", "assistant", "tool"]);
    assert_eq!(
        messages_seen[2]["tool_calls"][0]["function"]["name"],
        "read_file"
    );
    assert_eq!(messages_seen[2]["tool_calls"][0]["id"], "toolu_1");
    assert_eq!(messages_seen[3]["tool_call_id"], "toolu_1");
    assert_eq!(messages_seen[3]["content"], "fn main() {}");
    assert_eq!(bodies[0]["tools"][0]["function"]["name"], "read_file");

    // No key was asked for, nothing was spent, and the audit line says it ran here.
    let calls = world.accountd.as_ref().expect("accountd").calls();
    assert!(calls.resolved.is_empty() && calls.verdicts.is_empty());
    assert_eq!(spent(&world, "").micro_usd, 0);
    let audited = world.audit.entries();
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].locality, porter_core::Locality::OnDevice);
    assert_eq!(audited[0].cost, None);
    assert_eq!(audited[0].app, agent_app());
    assert!(provider_requests(&world).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_route_streams_openai_tool_calls_and_thinking_in_both_shapes() {
    let world = World::start(plan(
        vec![],
        say("unused"),
        Chat::ThinkSay(vec!["let me ", "see"], vec!["Hel", "lo"]),
    ))
    .await;
    let endpoint = open(
        &world,
        &model_route("tiny-chat", "any", &[]),
        "anthropic_messages",
    )
    .await;
    let reply = messages(&endpoint, &anthropic_body("claude-sonnet-4", true)).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let body = &reply.body;
    let at = |needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {body}"))
    };
    assert!(
        at("\"type\":\"thinking\"") < at("\"type\":\"text\""),
        "thinking comes first"
    );
    assert!(body.contains("thinking_delta") && body.contains("\"thinking\":\"let me see\""));
    assert!(body.contains("\"text\":\"Hel\"") && body.contains("\"text\":\"lo\""));
    assert!(at("message_delta") < at("message_stop"));
    assert!(
        body.contains("\"model\":\"claude-sonnet-4\""),
        "the id the agent named"
    );

    let reply = completions(
        &endpoint,
        &json!({
            "model": "gpt-whatever", "stream": true, "stream_options": { "include_usage": true },
            "messages": [{ "role": "user", "content": "hi" }],
        }),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(
        reply.body.contains("\"reasoning_content\":\"let me see\""),
        "{}",
        reply.body
    );
    assert!(reply.body.contains("\"content\":\"Hel\""), "{}", reply.body);
    assert!(
        reply.body.contains("\"finish_reason\":\"stop\""),
        "{}",
        reply.body
    );
    assert!(
        reply.body.contains("\"completion_tokens\":2"),
        "{}",
        reply.body
    );
    assert!(reply.body.trim_end().ends_with("data: [DONE]"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_route_names_its_model_and_an_other_id_is_an_error() {
    let world = World::start(plan(vec![], say("x"), say("hello"))).await;
    let own = open(
        &world,
        &model_route("tiny-chat", "listed", &[]),
        "openai_compatible",
    )
    .await;
    let body =
        |model: &str| json!({ "model": model, "messages": [{ "role": "user", "content": "hi" }] });
    assert_eq!(completions(&own, &body("tiny-chat")).await.status, 200);
    let other = completions(&own, &body("claude-sonnet-4")).await;
    assert_eq!(other.status, 404, "never a silent substitute");
    assert_eq!(other.json()["error"]["code"], "model_not_found");
    let listed = send(
        own.port,
        "GET",
        "/v1/models",
        &[("authorization", &format!("Bearer {}", own.token))],
        "",
    )
    .await;
    assert_eq!(listed.json()["data"][0]["id"], "tiny-chat");
    // An alias the launcher lists is answered by the route's model.
    let aliased = open(
        &world,
        &model_route("tiny-chat", "listed", &["claude-sonnet-4"]),
        "openai_compatible",
    )
    .await;
    assert_eq!(
        completions(&aliased, &body("claude-sonnet-4")).await.status,
        200
    );
    assert_eq!(completions(&aliased, &body("tiny-chat")).await.status, 404);
    // A model that is not there, and a class the model's place does not take.
    let refused = open_with(
        &world,
        &model_route("nonesuch", "listed", &[]),
        "files",
        &["openai_compatible"],
    )
    .await
    .expect_err("no model");
    assert_eq!(
        error_name(&refused),
        "org.quire.Inference1.Error.NoSuchRoute"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_of_only_reasoning_is_an_error_with_a_status_in_both_shapes() {
    let world = World::start(plan(
        vec![],
        say("x"),
        Chat::Think(vec!["secret ", "musings"]),
    ))
    .await;
    let endpoint = open(
        &world,
        &model_route("tiny-chat", "any", &[]),
        "anthropic_messages",
    )
    .await;
    for stream in [false, true] {
        let reply = messages(&endpoint, &anthropic_body("m", stream)).await;
        assert_eq!(reply.status, 502, "stream {stream}: {}", reply.body);
        let error = reply.json();
        assert_eq!(error["error"]["type"], "api_error");
        let text = error["error"]["message"].as_str().expect("message");
        assert!(
            text.contains("only reasoning") && text.contains("14 bytes"),
            "{text}"
        );
        assert!(
            !text.contains("secret") && !text.contains("musings"),
            "never the thought: {text}"
        );
        let reply = completions(
            &endpoint,
            &json!({ "model": "m", "stream": stream,
            "messages": [{ "role": "user", "content": "hi" }] }),
        )
        .await;
        assert_eq!(reply.status, 502, "stream {stream}: {}", reply.body);
        assert_eq!(reply.json()["error"]["code"], "only_reasoning");
    }
    assert_eq!(world.audit.entries().len(), 4, "each request is audited");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_attached_my_network_engine_follows_the_floor_and_the_my_network_setting() {
    use hosting::lab::Lab;
    use inferd::attached::{Attached, Place, Reach};
    use porter_fake_servers::net::Bind;
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("ag-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch");
    let lab = Lab::start(&Bind::Socket(dir.clone()), "lab", &[entries::SERVED], None).await;
    lab.say(&["from the lab"]);
    let mut p = Plan {
        catalog: vec![("attached.toml", entries::attached())],
        attached: vec![Attached {
            id: porter_core::ModelId::parse(entries::ATTACHED).expect("id"),
            reach: Reach::Socket(lab.socket()),
            key_file: None,
            place: Place::MyNetwork,
        }],
        role: Role::AgentLauncher,
        agents: Some(AgentsPlan {
            endpoint: AgentEndpoint::On,
        }),
        ..Plan::default()
    };
    p.policy = Policy::proposed();
    let world = World::start(p).await;
    let route = model_route(entries::ATTACHED, "listed", &[]);
    // The strict floor: on-device-only data does not go to another of the person's machines.
    let refused = open_with(&world, &route, "files", &["openai_compatible"])
        .await
        .expect_err("floor");
    assert_eq!(
        error_name(&refused),
        "org.quire.Inference1.Error.FloorRefused"
    );
    assert!(lab.chats().is_empty(), "nothing was sent");
    let mut settings = (*world.served.settings()).clone();
    settings.my_network = inferd::settings::MyNetwork::On;
    world.served.apply(settings);
    let endpoint = Endpoint::of(
        open_with(&world, &route, "files", &["openai_compatible"])
            .await
            .expect("my_network on"),
    );
    let ask =
        json!({ "model": entries::ATTACHED, "messages": [{ "role": "user", "content": "hi" }] });
    let reply = completions(&endpoint, &ask).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(
        reply.json()["choices"][0]["message"]["content"],
        "from the lab"
    );
    assert_eq!(lab.chats().len(), 1);
    // Free and keyless: no spend, and the audit line says it was another machine of theirs.
    let audited = world.audit.entries();
    assert_eq!(audited[0].locality, porter_core::Locality::LocalNetwork);
    assert_eq!(audited[0].cost, None);
    // The setting turned back off: the open session's next request is refused.
    let mut settings = (*world.served.settings()).clone();
    settings.my_network = inferd::settings::MyNetwork::Off;
    world.served.apply(settings);
    let reply = completions(&endpoint, &ask).await;
    assert_eq!(reply.status, 403, "{}", reply.body);
    assert_eq!(lab.chats().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
