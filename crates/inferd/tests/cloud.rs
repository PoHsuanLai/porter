//! Hosted models, end to end on a private bus: the real `Inference1` object, a fake accountd that
//! answers `Verdicts` and hands keys on sealed memfds, and a fake provider that speaks the
//! OpenAI-compatible chat API over TLS on loopback (the scratch CA, trusted through `Roots::Only`).
//! Acceptance 7 of design/31 section 7.1: no key is on any bus, in any file or in any audit line,
//! checked by a scan that has a positive control.

mod hosting;

use hosting::accountd::{FakeAccount, Standing};
use hosting::engine::{Chat, Script};
use hosting::entries;
use hosting::rig::{Hosted, Plan, Trust, World};
use inferd::settings::{ConfigFile, InferdSettings, Reload, SpendLine, serve_settings};
use porter_client::InferSession;
use porter_core::capability::LlmFeature;
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::{AppId, AppName, DataClass, Isolation, MicroUsd, Need, Tier, Tokens};
use porter_dbus::PeerProxy;
use porter_infer::{
    ChatControl, ChatMessage, ChatRequest, ClientFrame, InferEvent, InferRefusal, InferReply,
    InferRequest, Knob, LocalOnly, MessagePart, ModelError, Policy, Reasoning, ReplyShape,
    Role as ChatRole, StopReason, ToolChoice, ToolParallelism,
};
use std::path::Path;

/// OpenRouter's key, and OpenAI's: distinctive, so a scan for either cannot match by accident.
const KEY_OR: &str = "sk-or-v1-S3CRET-CLOUD-KEY-0123456789";
const KEY_OAI: &str = "sk-proj-OTHER-SECRET-KEY-9876543210";

const COMPANION: &str = "org.quire.Companion";
const READER: &str = "org.quire.Reader";

fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn llm() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(1000),
    })
}

fn chat_request(text: &str, class: DataClass) -> InferRequest {
    InferRequest::Chat(ChatRequest {
        messages: vec![ChatMessage {
            role: ChatRole::User,
            parts: vec![MessagePart::Text(text.into())],
        }],
        shape: ReplyShape::Text,
        tier: Tier::Balanced,
        class,
        usage: Usage::Interactive,
        tools: vec![],
        control: ChatControl {
            tool_choice: ToolChoice::Auto,
            tool_calls: ToolParallelism::One,
            max_output: Knob::Off,
            reasoning: Reasoning::EngineDefault,
            sampling: Knob::Off,
            stop: vec![],
        },
    })
}

async fn until_finished(session: &mut impl InferSession) -> Vec<InferEvent> {
    let mut events = Vec::new();
    loop {
        let event = session.next().await.expect("an event");
        let done = matches!(event, InferEvent::Finished(_));
        events.push(event);
        if done {
            return events;
        }
    }
}

/// One chat turn of the world's app, the events it produced.
async fn turn(world: &World, text: &str) -> Vec<InferEvent> {
    let mut session = world
        .accounts
        .session(&llm(), DataClass::Prompt, Tier::Balanced)
        .await
        .expect("open");
    // A refused session closes by itself: the send may find it already gone, and its one event
    // is still waiting to be read.
    let _ = session
        .send(ClientFrame::Request(chat_request(text, DataClass::Prompt)))
        .await;
    until_finished(&mut session).await
}

fn finished(events: &[InferEvent]) -> &InferReply {
    match events.last() {
        Some(InferEvent::Finished(reply)) => reply,
        other => panic!("a finished session, got {other:?}"),
    }
}

/// A person who allows cloud models for every class: the floors are lowered and local-only is off.
fn open_policy() -> Policy {
    Policy {
        local_only: LocalOnly::Off,
        floors: Vec::new(),
    }
}

fn openrouter(to: &[&'static str]) -> FakeAccount {
    FakeAccount {
        id: "openrouter",
        standing: Standing::Granted {
            to: to.to_vec(),
            grant: "grant-openrouter",
            key: Some(KEY_OR),
        },
    }
}

fn openai(to: &[&'static str]) -> FakeAccount {
    FakeAccount {
        id: "openai",
        standing: Standing::Granted {
            to: to.to_vec(),
            grant: "grant-openai",
            key: Some(KEY_OAI),
        },
    }
}

fn plan(accounts: Vec<FakeAccount>, app_name: &str) -> Plan {
    Plan {
        catalog: vec![
            ("a-claude-opus-5.5.toml", entries::claude()),
            ("b-gpt-6-luna.toml", entries::luna()),
            ("c-kimi-k3.toml", entries::kimi()),
        ],
        policy: open_policy(),
        app: Some(app(app_name)),
        hosted: Some(Hosted {
            accounts,
            chat: Script {
                chat: vec![Chat::Say(vec!["Hel", "lo"])],
                dims: 0,
            },
            trust: Trust::ScratchCa,
        }),
        ..Plan::default()
    }
}

fn routed(events: &[InferEvent]) -> &porter_infer::ServedBy {
    events
        .iter()
        .find_map(|event| match event {
            InferEvent::Routed(served) => Some(served),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a Routed event in {events:?}"))
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// Every message on the bus from the moment the tap is set: the bytes as a monitor sees them.
struct Tap(zbus::MessageStream);

impl Tap {
    async fn start(bus: &hosting::bus::PrivateBus) -> Self {
        let monitor = bus.connect().await;
        zbus::fdo::MonitoringProxy::new(&monitor)
            .await
            .expect("proxy")
            .become_monitor(&[], 0)
            .await
            .expect("monitor");
        Self(zbus::MessageStream::from(monitor))
    }

    async fn drain(&mut self) -> Vec<Vec<u8>> {
        use zbus::export::futures_core::Stream;
        let mut seen = Vec::new();
        loop {
            let next = tokio::time::timeout(
                std::time::Duration::from_millis(300),
                std::future::poll_fn(|cx| std::pin::Pin::new(&mut self.0).poll_next(cx)),
            )
            .await;
            match next {
                Ok(Some(Ok(message))) => seen.push(message.data().to_vec()),
                _ => return seen,
            }
        }
    }
}

/// Every file under `dir` that holds `needle`.
fn files_holding(dir: &Path, needle: &str) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            found.extend(files_holding(&path, needle));
        } else if kind.is_file() && std::fs::read(&path).is_ok_and(|bytes| contains(&bytes, needle))
        {
            found.push(path);
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread")]
async fn acceptance_7_a_chat_turn_goes_to_a_hosted_model_through_openrouter_and_no_key_is_on_the_bus()
 {
    let world = World::start(plan(vec![openrouter(&[COMPANION])], COMPANION)).await;
    let mut tap = Tap::start(&world.bus).await;

    let events = turn(&world, "hi there").await;

    // Routed to a hosted entry through the account of the grant, whose price decides among them.
    let served = routed(&events);
    assert_eq!(served.account.as_str(), "openrouter");
    assert_eq!(served.model.as_str(), "gpt-6-luna");
    assert!(matches!(
        served.locality,
        porter_core::Locality::Cloud { .. }
    ));
    let InferReply::Chat(reply) = finished(&events) else {
        panic!("a chat reply, got {events:?}");
    };
    assert_eq!(reply.text, "Hello");
    assert_eq!(reply.stop, StopReason::EndTurn);
    assert_eq!(
        (reply.usage.input, reply.usage.output),
        (Tokens(5), Tokens(2))
    );

    // What the provider saw: the key in Authorization, OpenRouter's name for the model, the base
    // path of the provider, and no temperature (the app chose none).
    let provider = world.provider.as_ref().expect("provider");
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/v1/chat/completions");
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {KEY_OR}").as_str())
    );
    assert_eq!(requests[0].body["model"], "openai/gpt-6-luna");
    assert!(
        requests[0].body.get("temperature").is_none(),
        "{}",
        requests[0].body
    );
    assert!(requests[0].body.to_string().contains("hi there"));

    // What accountd was asked: who the app is, and the one grant the key was resolved under.
    let calls = world.accountd.as_ref().expect("accountd").calls();
    assert_eq!(
        calls.verdicts,
        vec![(COMPANION.to_owned(), "prompt".to_owned())]
    );
    assert_eq!(calls.resolved, vec!["grant-openrouter".to_owned()]);

    // The turn was audited against the account that served it, and metered at the reach's price:
    // five tokens in and two out at 0.1 and 0.5 dollars a million is two micro-dollars.
    let audited = world.audit.entries();
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].account.as_str(), "openrouter");
    assert!(matches!(
        audited[0].locality,
        porter_core::Locality::Cloud { .. }
    ));
    let cloud = world.served.cloud().expect("cloud");
    let spent = cloud.ledger().spent(
        &porter_infer::SpendScope::App(app(COMPANION)),
        porter_infer::Period::Daily,
        cloud.now(),
    );
    assert_eq!(
        (spent.micro_usd, spent.tokens_in, spent.tokens_out),
        (2, 5, 2)
    );

    // Acceptance 7. The positive control first: a key sent as an argument on the bus is found by
    // the scan (a stranger asking accountd to resolve the key as if it were a grant id).
    let stranger = world.bus.connect().await;
    let _ = PeerProxy::new(&stranger)
        .await
        .expect("proxy")
        .resolve_key(KEY_OR)
        .await;
    let seen = tap.drain().await;
    assert!(
        seen.len() > 10,
        "the monitor saw the traffic: {}",
        seen.len()
    );
    let carrying: Vec<_> = seen.iter().filter(|m| contains(m, KEY_OR)).collect();
    assert_eq!(
        carrying.len(),
        1,
        "only the deliberate control carries the key"
    );
    assert!(
        carrying[0].windows(10).any(|w| w == b"ResolveKey"),
        "and it is the control's call"
    );
    assert!(
        seen.iter().any(|m| contains(m, "grant-openrouter")),
        "positive control: the scan sees values that do cross"
    );
    // Nor is it in an audit line, a file the daemon's world holds, or the engines' debug text.
    let lines = serde_json::to_string(&audited).expect("json");
    assert!(!lines.contains(KEY_OR), "{lines}");
    assert_eq!(
        files_holding(&world.scratch, KEY_OR),
        Vec::<std::path::PathBuf>::new()
    );
    assert!(!format!("{:?}", world.served).contains(KEY_OR));
}

#[tokio::test(flavor = "multi_thread")]
async fn each_turn_fetches_the_key_again_and_a_provider_gets_only_that_turns_copy() {
    let world = World::start(plan(vec![openrouter(&[COMPANION])], COMPANION)).await;
    turn(&world, "one").await;
    turn(&world, "two").await;
    let calls = world.accountd.as_ref().expect("accountd").calls();
    assert_eq!(calls.resolved.len(), 2, "{calls:?}");
    let requests = world.provider.as_ref().expect("provider").requests();
    assert_eq!(requests.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_direct_grant_beats_openrouter_and_the_key_is_that_accounts() {
    let accounts = vec![openrouter(&[COMPANION]), openai(&[COMPANION])];
    let world = World::start(plan(accounts, COMPANION)).await;
    let events = turn(&world, "hi").await;
    let served = routed(&events);
    assert_eq!(
        served.account.as_str(),
        "openai",
        "the company's own account"
    );
    assert_eq!(served.model.as_str(), "gpt-6-luna");
    let requests = world.provider.as_ref().expect("provider").requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/chat/completions");
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {KEY_OAI}").as_str())
    );
    assert_eq!(
        requests[0].body["model"], "gpt-6-luna",
        "OpenAI's own name for it"
    );
    let calls = world.accountd.as_ref().expect("accountd").calls();
    assert_eq!(
        calls.resolved,
        vec!["grant-openai".to_owned()],
        "the other key was never asked for"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_without_a_grant_is_never_served_a_cloud_model_even_with_the_floor_lowered() {
    // The Reader holds no grant; the Companion does. The floor is lowered and local-only is off.
    let world = World::start(plan(vec![openrouter(&[COMPANION])], READER)).await;
    let events = turn(&world, "secret").await;
    assert_eq!(
        events,
        vec![InferEvent::Finished(InferReply::Refused(
            InferRefusal::NeedsGrant
        ))]
    );
    assert!(
        world
            .provider
            .as_ref()
            .expect("provider")
            .requests()
            .is_empty()
    );
    let calls = world.accountd.as_ref().expect("accountd").calls();
    assert_eq!(
        calls.verdicts,
        vec![(READER.to_owned(), "prompt".to_owned())]
    );
    assert_eq!(calls.resolved, Vec::<String>::new(), "no key was fetched");
    assert!(world.audit.entries().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_the_person_denied_the_app_is_a_refusal_and_no_account_is_unavailable() {
    let denied = FakeAccount {
        id: "openrouter",
        standing: Standing::Denied,
    };
    let world = World::start(plan(vec![denied], COMPANION)).await;
    assert_eq!(
        turn(&world, "x").await,
        vec![InferEvent::Finished(InferReply::Refused(
            InferRefusal::Denied
        ))]
    );
    let none = World::start(plan(Vec::new(), COMPANION)).await;
    assert_eq!(
        turn(&none, "x").await,
        vec![InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unavailable
        ))]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_floor_of_on_device_refuses_a_cloud_model_the_app_is_granted() {
    let mut world_plan = plan(vec![openrouter(&[COMPANION])], COMPANION);
    world_plan.policy = Policy {
        local_only: LocalOnly::Off,
        floors: vec![porter_infer::ClassFloor {
            class: DataClass::Prompt,
            floor: porter_infer::Floor::OnDevice,
        }],
    };
    let world = World::start(world_plan).await;
    assert_eq!(
        turn(&world, "private").await,
        vec![InferEvent::Finished(InferReply::Refused(
            InferRefusal::RequiresCloud(DataClass::Prompt)
        ))]
    );
    assert!(
        world
            .provider
            .as_ref()
            .expect("provider")
            .requests()
            .is_empty()
    );
    assert_eq!(
        world.accountd.as_ref().expect("accountd").calls().resolved,
        Vec::<String>::new()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn local_only_refuses_a_cloud_model_the_app_is_granted() {
    let mut world_plan = plan(vec![openrouter(&[COMPANION])], COMPANION);
    world_plan.policy = Policy::proposed();
    let world = World::start(world_plan).await;
    assert_eq!(
        turn(&world, "x").await,
        vec![InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unavailable
        ))]
    );
    assert!(
        world
            .provider
            .as_ref()
            .expect("provider")
            .requests()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_accountd_will_not_give_is_unauthorized_and_nothing_is_sent() {
    let refused = FakeAccount {
        id: "openrouter",
        standing: Standing::Granted {
            to: vec![COMPANION],
            grant: "grant-openrouter",
            key: None,
        },
    };
    let world = World::start(plan(vec![refused], COMPANION)).await;
    let events = turn(&world, "x").await;
    assert_eq!(
        finished(&events),
        &InferReply::Failed(ModelError::Unauthorized),
        "{events:?}"
    );
    assert!(
        world
            .provider
            .as_ref()
            .expect("provider")
            .requests()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_the_roots_do_not_vouch_for_gets_no_request_and_no_key() {
    let mut world_plan = plan(vec![openrouter(&[COMPANION])], COMPANION);
    if let Some(hosted) = world_plan.hosted.as_mut() {
        hosted.trust = Trust::NoOne;
    }
    let world = World::start(world_plan).await;
    let events = turn(&world, "x").await;
    assert_eq!(
        finished(&events),
        &InferReply::Failed(ModelError::Unreachable),
        "{events:?}"
    );
    assert!(
        world
            .provider
            .as_ref()
            .expect("provider")
            .requests()
            .is_empty(),
        "the handshake failed before any request was written"
    );
    assert_eq!(
        world.audit.entries().len(),
        1,
        "a failed turn is still audited, without usage"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_that_rejects_the_key_is_unauthorized_and_is_not_charged() {
    let mut world_plan = plan(vec![openrouter(&[COMPANION])], COMPANION);
    if let Some(hosted) = world_plan.hosted.as_mut() {
        hosted.chat = Script {
            chat: vec![Chat::Fail(401)],
            dims: 0,
        };
    }
    let world = World::start(world_plan).await;
    let events = turn(&world, "x").await;
    assert_eq!(
        finished(&events),
        &InferReply::Failed(ModelError::Unauthorized),
        "{events:?}"
    );
    let cloud = world.served.cloud().expect("cloud");
    let spent = cloud.ledger().spent(
        &porter_infer::SpendScope::App(app(COMPANION)),
        porter_infer::Period::Daily,
        cloud.now(),
    );
    assert_eq!(spent.micro_usd, 0);
}

fn tools_request() -> InferRequest {
    let InferRequest::Chat(chat) = chat_request("look it up", DataClass::Prompt) else {
        unreachable!("a chat request");
    };
    InferRequest::Chat(ChatRequest {
        tools: vec![porter_infer::ToolDecl {
            name: porter_infer::ToolName::parse("lookup").expect("name"),
            description: "look something up".into(),
            params: porter_infer::JsonSchemaText(
                porter_infer::JsonText::parse(r#"{"type":"object","properties":{}}"#)
                    .expect("json"),
            ),
        }],
        ..chat
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn luna_with_tools_is_told_to_reason_off_in_the_provider_spelling_and_without_tools_it_is_not()
 {
    for (accounts, field, want) in [
        (
            vec![openai(&[COMPANION])],
            "reasoning_effort",
            serde_json::json!("none"),
        ),
        (
            vec![openrouter(&[COMPANION])],
            "reasoning",
            serde_json::json!({"effort": "none"}),
        ),
    ] {
        let world = World::start(plan(accounts, COMPANION)).await;
        let mut session = world
            .accounts
            .session(&llm(), DataClass::Prompt, Tier::Balanced)
            .await
            .expect("open");
        session
            .send(ClientFrame::Request(tools_request()))
            .await
            .expect("send");
        until_finished(&mut session).await;
        session
            .send(ClientFrame::Request(chat_request(
                "plain",
                DataClass::Prompt,
            )))
            .await
            .expect("send");
        until_finished(&mut session).await;
        let requests = world.provider.as_ref().expect("provider").requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].body[field], want, "{}", requests[0].body);
        assert!(
            requests[1].body.get(field).is_none(),
            "{}",
            requests[1].body
        );
    }
}

fn capped(cents: i64) -> SpendLine {
    let config = inferd::config::InferdConfig::from_toml(&format!(
        "[ai.spend]\napp_daily_cents = {cents}\n"
    ))
    .expect("config");
    inferd::settings::resolve(&config).settings.spend
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cap_the_app_has_reached_refuses_with_the_spend_refusal_and_nothing_is_sent() {
    let mut world_plan = plan(vec![openrouter(&[COMPANION])], COMPANION);
    world_plan.spend = capped(1);
    let world = World::start(world_plan).await;
    let cloud = world.served.cloud().expect("cloud");
    // One cent is ten thousand micro-dollars; the app has spent that today.
    cloud.ledger().record(
        &app(COMPANION),
        &porter_core::AccountId::parse("openrouter").expect("id"),
        porter_infer::TokenUsage {
            input: Tokens(0),
            output: Tokens(0),
            cached: Tokens(0),
        },
        MicroUsd(10_000),
        cloud.now(),
    );
    assert_eq!(
        turn(&world, "x").await,
        vec![InferEvent::Finished(InferReply::Refused(
            InferRefusal::OverBudget
        ))]
    );
    assert!(
        world
            .provider
            .as_ref()
            .expect("provider")
            .requests()
            .is_empty()
    );
    assert_eq!(
        world.accountd.as_ref().expect("accountd").calls().resolved,
        Vec::<String>::new()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_warning_line_still_serves_the_turn_and_a_cap_met_mid_session_stops_the_next_one() {
    let mut world_plan = plan(vec![openrouter(&[COMPANION])], COMPANION);
    world_plan.spend = capped(1);
    let world = World::start(world_plan).await;
    let cloud = world.served.cloud().expect("cloud").clone();
    let record = |micro_usd| {
        cloud.ledger().record(
            &app(COMPANION),
            &porter_core::AccountId::parse("openrouter").expect("id"),
            porter_infer::TokenUsage {
                input: Tokens(0),
                output: Tokens(0),
                cached: Tokens(0),
            },
            MicroUsd(micro_usd),
            cloud.now(),
        );
    };
    // Past the warning line (eight thousand of ten thousand), under the cap: served.
    record(8_000);
    let mut session = world
        .accounts
        .session(&llm(), DataClass::Prompt, Tier::Balanced)
        .await
        .expect("open");
    session
        .send(ClientFrame::Request(chat_request("one", DataClass::Prompt)))
        .await
        .expect("send");
    let first = until_finished(&mut session).await;
    assert!(matches!(finished(&first), InferReply::Chat(_)), "{first:?}");
    // The same session, after something else spent the rest: the next turn is refused.
    record(2_000);
    session
        .send(ClientFrame::Request(chat_request("two", DataClass::Prompt)))
        .await
        .expect("send");
    let second = until_finished(&mut session).await;
    assert_eq!(
        finished(&second),
        &InferReply::Refused(InferRefusal::OverBudget)
    );
    assert_eq!(
        world.provider.as_ref().expect("provider").requests().len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn usage_over_the_bus_says_what_the_caller_spent_and_what_it_may() {
    let mut world_plan = plan(vec![openrouter(&[COMPANION])], COMPANION);
    world_plan.spend = capped(5);
    let world = World::start(world_plan).await;
    turn(&world, "x").await;
    let usage = porter_dbus::InferenceProxy::new(&world.client)
        .await
        .expect("proxy")
        .usage()
        .await
        .expect("usage");
    let number = |name: &str| -> u64 {
        u64::try_from(
            usage
                .get(name)
                .unwrap_or_else(|| panic!("{name} in {usage:?}"))
                .try_clone()
                .expect("clone"),
        )
        .expect("a number")
    };
    assert_eq!(number("tokens_in_day"), 5);
    assert_eq!(number("tokens_out_day"), 2);
    assert_eq!(number("spend_day_micro_usd"), 2);
    assert_eq!(number("cap_day_micro_usd"), 50_000);
    assert!(
        !usage.contains_key("cap_month_micro_usd"),
        "no monthly cap was set"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn availability_says_granted_for_an_app_with_a_grant_and_needs_consent_for_one_without() {
    for (who, want) in [(COMPANION, "granted"), (READER, "available_needs_consent")] {
        let world = World::start(plan(vec![openrouter(&[COMPANION])], who)).await;
        let got = porter_dbus::InferenceProxy::new(&world.client)
            .await
            .expect("proxy")
            .availability(
                &porter_dbus::need_to_dbus(&llm()),
                "prompt",
                &porter_dbus::Details::new(),
            )
            .await
            .expect("availability");
        assert_eq!(got, want, "{who}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_picker_lists_every_hosted_model_by_company_and_marks_the_ones_no_account_reaches() {
    use ds_settings::live::LiveClient;
    use ds_settings::schema::{KeyKind, KeyPath};
    use inferd::peers::{Caller, Role};

    for (accounts, reachable, unreachable) in [
        (
            vec![openai(&[COMPANION])],
            vec!["cloud/gpt-6-luna"],
            vec!["cloud/claude-opus-5.5", "cloud/kimi-k3"],
        ),
        (
            vec![openrouter(&[COMPANION])],
            vec!["cloud/claude-opus-5.5", "cloud/gpt-6-luna", "cloud/kimi-k3"],
            vec![],
        ),
        (
            Vec::new(),
            vec![],
            vec!["cloud/claude-opus-5.5", "cloud/gpt-6-luna", "cloud/kimi-k3"],
        ),
    ] {
        let world = World::start(plan(accounts, COMPANION)).await;
        let file = world.scratch.join("inferd.toml");
        let reload = Reload::new(ConfigFile::new(file), world.served.clone());
        serve_settings(
            &world.daemon,
            InferdSettings::new(std::sync::Arc::clone(&world.peers), reload),
        )
        .await
        .expect("serve the module");
        let settings = world.bus.connect().await;
        world.peers.introduce(
            settings.unique_name().expect("name").as_str(),
            Caller {
                app: app("org.quire.Settings"),
                role: Role::Settings,
            },
        );
        let live = LiveClient::new(
            &settings,
            porter_dbus::INFERENCE_BUS,
            porter_dbus::INFERENCE_SETTINGS_PATH,
        )
        .await
        .expect("client");
        let schema = live.describe().await.expect("schema");
        schema.check().expect("a clean schema");
        let row = schema
            .key
            .iter()
            .find(|k| k.path.0 == "ai.model.text.balanced")
            .expect("the language row");
        let KeyKind::Menu { variants } = &row.kind else {
            panic!("a menu, got {:?}", row.kind);
        };
        let hosted: Vec<&str> = variants
            .iter()
            .filter(|v| v.starts_with("cloud/"))
            .map(String::as_str)
            .collect();
        assert_eq!(
            hosted,
            ["cloud/claude-opus-5.5", "cloud/gpt-6-luna", "cloud/kimi-k3"],
            "every curated model is listed, in catalogue order, grouped by company"
        );
        // Every slot a hosted model fits has its own picker row, grouped the same way.
        let image_row = schema
            .key
            .iter()
            .find(|k| k.path.0 == "ai.model.image_in.balanced")
            .expect("the image row");
        let KeyKind::Menu {
            variants: image_variants,
        } = &image_row.kind
        else {
            panic!("a menu, got {:?}", image_row.kind);
        };
        assert!(image_variants.iter().any(|v| v == "cloud/claude-opus-5.5"));
        for value in &reachable {
            let label = &row.labels.0[*value];
            assert!(!label.contains("Add an account to use"), "{value}: {label}");
        }
        for value in &unreachable {
            let label = &row.labels.0[*value];
            assert!(label.ends_with("Add an account to use"), "{value}: {label}");
        }
        // Choosing one, whatever the accounts, is a value the row accepts.
        live.set(
            &KeyPath("ai.model.text.balanced".into()),
            &toml::Value::String("cloud/kimi-k3".into()),
        )
        .await
        .expect("a hosted model is a value");
        assert_eq!(
            live.get(&KeyPath("ai.model.text.balanced".into()))
                .await
                .expect("get"),
            toml::Value::String("cloud/kimi-k3".into())
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_picked_hosted_model_is_served_through_the_account_the_app_is_granted() {
    let accounts = vec![openrouter(&[COMPANION]), openai(&[COMPANION])];
    let world = World::start(plan(accounts, COMPANION)).await;
    // The person picked Kimi K3 for balanced work: `cloud/kimi-k3`, an account of nobody's.
    let mut settings = world.served.settings().as_ref().clone();
    settings.tiers.rows.push(porter_infer::TierRow {
        kind: porter_infer::Slot::Text,
        tier: Tier::Balanced,
        model: porter_infer::ModelRef {
            account: porter_core::AccountId::parse("cloud").expect("id"),
            model: porter_core::ModelId::parse("kimi-k3").expect("id"),
        },
    });
    world.served.apply(settings);
    let events = turn(&world, "hi").await;
    let served = routed(&events);
    assert_eq!(
        (served.account.as_str(), served.model.as_str()),
        ("openrouter", "kimi-k3"),
        "Moonshot has no account here, so the gateway's"
    );
    let requests = world.provider.as_ref().expect("provider").requests();
    assert_eq!(requests[0].body["model"], "moonshotai/kimi-k3");
    assert!(
        events.iter().any(|event| matches!(
            event,
            InferEvent::Why(porter_infer::Why::Reached {
                provider,
                door: porter_infer::Door::Gateway,
            }) if provider.0 == "openrouter"
        )),
        "the footer can say it came via OpenRouter: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_asks_accountd_for_the_usage_it_was_opened_with() {
    let world = World::start(plan(vec![openrouter(&[COMPANION])], COMPANION)).await;
    for usage in [Usage::Background, Usage::Interactive] {
        let offered = world
            .served
            .offer(&app(COMPANION), DataClass::Prompt, usage)
            .await;
        assert_eq!(offered.models().count(), 3, "{usage:?}");
    }
    let calls = world.accountd.as_ref().expect("accountd").calls();
    assert_eq!(
        calls.usages,
        vec!["background".to_owned(), "interactive".to_owned()]
    );
}
