//! A server of the person's own (an account of `openai-compatible`) end to end on a private bus:
//! the account's address is a TLS fake on loopback (the scratch CA, trusted through
//! `TlsRoots::Only`) that lists two models and speaks the chat protocol. Its models are listed
//! the first time the account is seen and again on `relist`, offered as hosted models with no
//! limit known, and a turn reaches the server at its own address under its own name for the
//! model, with the account's key as a bearer.

use crate::cloud::{COMPANION, app, finished, open_policy, routed, turn};
use crate::hosting;
use hosting::accountd::FakeServer;
use hosting::engine::{Chat, Script};
use hosting::rig::{Hosted, Plan, Trust, World};
use porter_core::DataClass;
use porter_core::consent::Usage;
use porter_infer::{InferReply, StopReason};

const KEY_OWN: &str = "sk-own-S3CRET-SERVER-KEY-0042";

fn plan() -> Plan {
    Plan {
        policy: open_policy(),
        app: Some(app(COMPANION)),
        hosted: Some(Hosted {
            accounts: Vec::new(),
            chat: Script {
                chat: vec![Chat::Say(vec!["Hel", "lo"])],
                dims: 0,
            },
            trust: Trust::ScratchCa,
        }),
        pooled: vec!["my-llm/llama-3.2-3b"],
        ..Plan::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_servers_models_are_listed_for_the_account_and_a_turn_reaches_it() {
    let world = World::start(plan()).await;
    let provider = world.provider.as_ref().expect("provider");
    world
        .accountd
        .as_ref()
        .expect("accountd")
        .add_server(FakeServer {
            id: "my-llm",
            label: "Studio",
            to: vec![COMPANION],
            grant: "grant-my-llm",
            key: Some(KEY_OWN),
            url: format!("https://localhost:{}/compat/v1", provider.port),
        });

    // Its models are hosted models of its account, with no context window or longest reply known.
    let cloud = world.served.cloud().expect("cloud");
    let models = cloud
        .models(&app(COMPANION), DataClass::Prompt, Usage::Interactive)
        .await;
    let mut listed: Vec<(String, String, String)> = models
        .iter()
        .map(|m| {
            (
                m.card.account.to_string(),
                m.card.model.to_string(),
                m.entry.label.clone(),
            )
        })
        .collect();
    listed.sort();
    let want = |model: &str, label: &str| ("my-llm".to_owned(), model.to_owned(), label.to_owned());
    assert_eq!(
        listed,
        [want("llama-3.2-3b", "Llama-3.2:3B"), want("qwen3", "qwen3")]
    );
    for model in &models {
        let text = model.entry.capabilities.text_out.as_ref().expect("text");
        assert_eq!((text.context.0, text.max_output.0), (0, 0));
        assert!(model.grant().is_some());
    }

    // A turn goes to the model it was routed to, under the server's own name for it.
    let events = turn(&world, "hi there").await;
    let served = routed(&events);
    assert_eq!(served.account.as_str(), "my-llm");
    assert_eq!(served.model.as_str(), "llama-3.2-3b");
    assert!(matches!(
        served.locality,
        porter_core::Locality::Cloud { .. }
    ));
    let InferReply::Chat(reply) = finished(&events) else {
        panic!("a chat reply, got {events:?}");
    };
    assert_eq!(reply.text, "Hello");
    assert_eq!(reply.stop, StopReason::EndTurn);

    // What the server saw: one list, one chat, each at its base path with the key as a bearer,
    // and the model by the name the server gave it.
    let bearer = format!("Bearer {KEY_OWN}");
    let requests = provider.requests();
    let (lists, chats): (Vec<_>, Vec<_>) = requests.iter().partition(|seen| seen.method == "GET");
    assert_eq!(lists.len(), 1, "the list is asked for once: {requests:?}");
    assert_eq!(lists[0].path, "/compat/v1/models");
    assert_eq!(lists[0].header("authorization"), Some(bearer.as_str()));
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].path, "/compat/v1/chat/completions");
    assert_eq!(chats[0].header("authorization"), Some(bearer.as_str()));
    assert_eq!(chats[0].body["model"], "Llama-3.2:3B");
    assert!(chats[0].body.to_string().contains("hi there"));

    // Rescan asks the server again.
    cloud.relist().await;
    let lists_after = provider
        .requests()
        .iter()
        .filter(|seen| seen.method == "GET")
        .count();
    assert_eq!(lists_after, 2);
}
