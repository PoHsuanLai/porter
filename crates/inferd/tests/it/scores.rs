//! How likely each option of a `Choice` was, end to end on a private bus: the real
//! `Inference1` object, a client opening sessions through `porter-client`, and a fake
//! OpenAI-compatible engine that sends (or does not send) the first token's log-probabilities as
//! vLLM and llama-server do. The reply is the same reply whatever the engine gives: its scores are
//! there when it gave usable ones and `None` when it did not, never a failed turn.

use crate::hosting;

use hosting::agent::{completions, model_route, open, plan, say};
use hosting::bus::within;
use hosting::engine::{Chat, Script};
use hosting::entries;
use hosting::rig::{Plan, World};
use porter_client::InferSession;
use porter_core::capability::LlmFeature;
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::{DataClass, Need, Tier, Tokens};
use porter_infer::{
    ChatControl, ChatMessage, ChatReply, ChatRequest, ClientFrame, InferEvent, InferReply,
    InferRequest, Knob, MessagePart, Reasoning, ReplyShape, Role as ChatRole, ScoreOptions,
};
use serde_json::json;

fn llm() -> Need {
    Need::Llm(LlmNeed::new([LlmFeature::Chat].into(), Tokens(1000)))
}

/// The chat entry, whose engine also constrains a reply to a list of strings (`choice`), so a
/// `Choice` is asked of it natively.
fn chat_choice() -> String {
    entries::chat().replace(
        r#"output = ["json_schema"]"#,
        r#"output = ["json_schema", "choice"]"#,
    )
}

/// A world whose one chat model constrains a reply to a list of strings and says `answer`.
fn choice_world(answer: Chat) -> Plan {
    Plan {
        catalog: vec![("tiny-chat.toml", chat_choice())],
        scripts: vec![(
            "tiny-chat",
            Script {
                chat: vec![answer],
                dims: 0,
            },
        )],
        ..Plan::default()
    }
}

fn pick_one(options: &[&str], scores: Knob<ScoreOptions>) -> InferRequest {
    InferRequest::Chat(
        ChatRequest::new(
            vec![ChatMessage {
                role: ChatRole::User,
                parts: vec![MessagePart::Text("may it run?".into())],
            }],
            Tier::Balanced,
            DataClass::Notes,
            Usage::Interactive,
        )
        .with_shape(ReplyShape::Choice(
            options.iter().map(|one| (*one).to_owned()).collect(),
        ))
        .with_control(
            ChatControl::new()
                .with_reasoning(Reasoning::Off)
                .with_scores(scores),
        ),
    )
}

async fn ask(world: &World, request: InferRequest) -> ChatReply {
    let mut session = within(
        "the session to open",
        world
            .accounts
            .session(&llm(), DataClass::Notes, Tier::Balanced),
    )
    .await
    .expect("open");
    within(
        "the request to be sent",
        session.send(ClientFrame::Request(request)),
    )
    .await
    .expect("send");
    within("the chat reply to finish", async {
        loop {
            match session.next().await.expect("an event") {
                InferEvent::Finished(InferReply::Chat(reply)) => return reply,
                InferEvent::Finished(other) => panic!("a chat reply, got {other:?}"),
                _ => {}
            }
        }
    })
    .await
}

fn shares(reply: &ChatReply) -> Vec<(&str, u32)> {
    reply
        .scores
        .as_ref()
        .expect("scores")
        .as_slice()
        .iter()
        .map(|one| (one.option.as_str(), one.share.0))
        .collect()
}

fn on() -> Knob<ScoreOptions> {
    Knob::Set(ScoreOptions::default())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_choice_that_asks_gets_each_options_share_and_the_engine_was_asked_for_first_tokens() {
    let world = World::start(choice_world(Chat::Choose {
        text: "allow",
        top: vec![("allow", 0.7), ("deny", 0.2), ("Sure", 0.05)],
    }))
    .await;
    let reply = ask(&world, pick_one(&["allow", "deny"], on())).await;
    assert_eq!(reply.text, "allow");
    // 0.7 and 0.2 of what is seen, a token that starts no option left out: 77.8 and 22.2 per
    // cent, the spare thousandth to the larger remainder.
    assert_eq!(shares(&reply), vec![("allow", 778), ("deny", 222)]);

    let bodies = world.engines["tiny-chat"].bodies("/v1/chat/completions");
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["logprobs"], json!(true), "{}", bodies[0]);
    assert_eq!(bodies[0]["top_logprobs"], json!(20), "{}", bodies[0]);
}

/// M1: nothing asked, nothing returned. Each row has its own world.
#[tokio::test(flavor = "multi_thread")]
async fn when_no_scores_are_asked_for_the_engine_is_asked_for_none_and_the_reply_has_none() {
    // (why, the knob, whether the reply is plain text instead of a choice, the text expected)
    let rows = [
        (
            "the same choice without the knob",
            false,
            false,
            Some("allow"),
        ),
        ("a reply that is not a choice", true, true, None),
    ];
    for (why, asked, as_text, text) in rows {
        let world = World::start(choice_world(Chat::Choose {
            text: "allow",
            top: vec![("allow", 0.7), ("deny", 0.2)],
        }))
        .await;
        let knob = if asked { on() } else { Knob::Off };
        let mut request = pick_one(&["allow", "deny"], knob);
        if let InferRequest::Chat(chat) = &mut request
            && as_text
        {
            chat.shape = ReplyShape::Text;
        }
        let reply = ask(&world, request).await;
        if let Some(text) = text {
            assert_eq!(reply.text, text, "{why}");
        }
        assert_eq!(reply.scores, None, "{why}");
        let bodies = world.engines["tiny-chat"].bodies("/v1/chat/completions");
        assert!(bodies[0].get("logprobs").is_none(), "{why}: {}", bodies[0]);
        assert!(
            bodies[0].get("top_logprobs").is_none(),
            "{why}: {}",
            bodies[0]
        );
    }
}

/// M1: scores asked for and none usable come back: the same reply without them, never a failed
/// turn. Each row has its own world.
#[tokio::test(flavor = "multi_thread")]
async fn scores_asked_for_and_none_usable_give_the_same_reply_without_them() {
    // (why, what the engine says, the options, the text expected, whether asking must change
    // nothing else in the reply)
    let rows = [
        (
            "an engine without log probabilities",
            Chat::Say(vec!["deny"]),
            vec!["allow", "deny"],
            "deny",
            true,
        ),
        (
            "a garbled log probability answer",
            Chat::ChooseGarbled("deny"),
            vec!["allow", "deny"],
            "deny",
            false,
        ),
        (
            "options that share a first token",
            Chat::Choose {
                text: "Allow",
                top: vec![("Al", 0.7), ("Deny", 0.2)],
            },
            vec!["Allow", "Always", "Deny"],
            "Allow",
            false,
        ),
    ];
    for (why, engine, options, text, same_as_plain) in rows {
        let world = World::start(choice_world(engine)).await;
        let asked = ask(&world, pick_one(&options, on())).await;
        assert_eq!(asked.text, text, "{why}");
        assert_eq!(asked.scores, None, "{why}");
        if same_as_plain {
            let plain = ask(&world, pick_one(&options, Knob::Off)).await;
            assert_eq!(
                asked, plain,
                "{why}: asking changed nothing else in the reply"
            );
        }
    }
}

// ---- the OpenAI-compatible front --------------------------------------------------------------

fn front_world(answer: Chat) -> Plan {
    let mut plan = plan(vec![], say("unused"), answer);
    plan.catalog[2] = ("tiny-chat.toml", chat_choice());
    plan
}

fn guided(logprobs: bool) -> serde_json::Value {
    json!({
        "model": "any", "stream": false, "logprobs": logprobs, "top_logprobs": 2,
        "guided_choice": ["allow", "deny"],
        "messages": [{ "role": "user", "content": "may it run?" }],
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn the_openai_front_returns_a_choices_shares_as_logprobs_when_asked_and_not_otherwise() {
    let world = World::start(front_world(Chat::Choose {
        text: "allow",
        top: vec![("allow", 0.7), ("deny", 0.2)],
    }))
    .await;
    let endpoint = open(
        &world,
        &model_route("tiny-chat", "any", &[]),
        "anthropic_messages",
    )
    .await;

    let reply = completions(&endpoint, &guided(true)).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let choice = &reply.json()["choices"][0];
    assert_eq!(choice["message"]["content"], "allow");
    let chosen = &choice["logprobs"]["content"][0];
    assert_eq!(chosen["token"], "allow");
    let near = |value: &serde_json::Value, share: f64| {
        (value.as_f64().expect("a number") - (share / 1000.0).ln()).abs() < 1e-9
    };
    assert!(near(&chosen["logprob"], 778.0), "{chosen}");
    let top = chosen["top_logprobs"].as_array().expect("a list");
    assert_eq!(top.len(), 2, "{chosen}");
    assert_eq!(top[0]["token"], "allow");
    assert_eq!(top[1]["token"], "deny");
    assert!(near(&top[1]["logprob"], 222.0), "{chosen}");

    // Without `logprobs` the reply is what it was: no key.
    let reply = completions(&endpoint, &guided(false)).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(
        reply.json()["choices"][0].get("logprobs").is_none(),
        "{}",
        reply.body
    );
    // `logprobs` on a request that is not a Choice is dropped without a word, as before.
    let plain = json!({
        "model": "any", "logprobs": true, "top_logprobs": 2,
        "messages": [{ "role": "user", "content": "hi" }],
    });
    let reply = completions(&endpoint, &plain).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(
        reply.json()["choices"][0].get("logprobs").is_none(),
        "{}",
        reply.body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_openai_front_says_null_logprobs_when_the_engine_gave_none() {
    let world = World::start(front_world(Chat::Say(vec!["deny"]))).await;
    let endpoint = open(
        &world,
        &model_route("tiny-chat", "any", &[]),
        "anthropic_messages",
    )
    .await;
    let reply = completions(&endpoint, &guided(true)).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let choice = &reply.json()["choices"][0];
    assert_eq!(choice["message"]["content"], "deny");
    assert!(choice["logprobs"].is_null(), "{}", reply.body);
}
