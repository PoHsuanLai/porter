//! Local runtimes as accounts, end to end on a private bus: the real `Inference1` object, a fake
//! accountd that hears `Peer.ReportLocal`, a fake Ollama on an ephemeral loopback port (never a
//! real runtime and never a port this test did not bind), and a fake hosted provider that is
//! granted to the app so that a call to it would be seen.
//!
//! Acceptance 6 of design/31 section 7.1: with `ai.local_only` on, a Mail-class summary from
//! `org.quire.Mail` runs on the probed Ollama, and with the Ollama stopped it is `Unavailable`
//! and no cloud call is made.

use crate::hosting;

use hosting::accountd::{FakeAccount, Standing};
use hosting::engine::{Chat, Script};
use hosting::entries;
use hosting::rig::{Hosted, Plan, Trust, World};
use inferd::probe::ProbeConfig;
use inferd::settings::Settings;
use porter_client::InferSession;
use porter_core::capability::LlmFeature;
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::{AppId, AppName, DataClass, Isolation, Locality, Need, Tier, Tokens};
use porter_dbus::InferenceProxy;
use porter_fake_servers::net::Bind;
use porter_fake_servers::{FakeModels, ModelDef, ModelsHandle, Running, Wire};
use porter_infer::{
    ChatMessage, ChatRequest, ClientFrame, InferEvent, InferRefusal, InferReply, InferRequest,
    LocalOnly, MessagePart, Policy, Role as ChatRole,
};
use std::time::Duration;

const MAIL: &str = "org.quire.Mail";
const KEY: &str = "sk-or-v1-NOT-USED-LOCAL-ONLY";

fn mail() -> AppId {
    AppId {
        name: AppName::parse(MAIL).expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn llm() -> Need {
    Need::Llm(LlmNeed::new([LlmFeature::Chat].into(), Tokens(1000)))
}

fn summarise(text: &str) -> InferRequest {
    InferRequest::Chat(ChatRequest::new(
        vec![ChatMessage {
            role: ChatRole::User,
            parts: vec![MessagePart::Text(text.into())],
        }],
        Tier::Balanced,
        DataClass::Mail,
        Usage::Interactive,
    ))
}

/// One Mail-class summary of the world's app, the events it produced.
async fn turn(world: &World, text: &str) -> Vec<InferEvent> {
    let mut session = world
        .accounts
        .session(&llm(), DataClass::Mail, Tier::Balanced)
        .await
        .expect("open");
    let _ = session.send(ClientFrame::Request(summarise(text))).await;
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

fn routed(events: &[InferEvent]) -> &porter_infer::ServedBy {
    events
        .iter()
        .find_map(|event| match event {
            InferEvent::Routed(served) => Some(served),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a Routed event in {events:?}"))
}

fn policy(local_only: LocalOnly) -> Policy {
    // The floors are lowered: only `local_only` stands between Mail data and the cloud.
    Policy {
        local_only,
        floors: Vec::new(),
    }
}

async fn fake_ollama(bind: &Bind) -> Running<ModelsHandle> {
    let fake = FakeModels::bind_on(
        bind,
        Wire::Ollama,
        vec![ModelDef::chat("llama3.2:3b", 8192)],
        None,
    )
    .await
    .expect("bind");
    let handle = fake.handle();
    handle.say(&["Two ", "lines."]);
    Running::spawn(fake, handle)
}

fn port_of(fake: &ModelsHandle) -> u16 {
    fake.base_url()
        .rsplit(':')
        .next()
        .expect("port")
        .parse()
        .expect("number")
}

/// A loopback port that nothing answers on and nothing else can take: a socket bound to it and
/// not listening, so a connect is refused. A fake that comes up on the port later can bind it,
/// since both set `SO_REUSEADDR` and this one does not listen. Hold the socket for the test.
fn reserved_port() -> (tokio::net::TcpSocket, u16) {
    let socket = tokio::net::TcpSocket::new_v4().expect("socket");
    socket.set_reuseaddr(true).expect("reuse the address");
    socket
        .bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("bind an ephemeral port");
    let port = socket.local_addr().expect("local address").port();
    (socket, port)
}

/// Stops a fake and gives its listener the moment it needs to close.
async fn stop<T>(fake: T) {
    drop(fake);
    tokio::time::sleep(Duration::from_millis(100)).await;
}

fn only_ollama(port: u16) -> ProbeConfig {
    ProbeConfig {
        ollama: vec![port],
        // The timer never fires in a test: only the start and `Rescan` look.
        every_s: 3600,
        longest_s: 3600,
        ..ProbeConfig::off()
    }
}

/// A world whose app is Mail, with OpenRouter granted to it (a cloud reach that would answer),
/// and a probe of one port for Ollama.
async fn world(port: u16, local_only: LocalOnly) -> World {
    World::start(Plan {
        catalog: vec![
            ("a-claude-opus-5.5.toml", entries::claude()),
            ("b-gpt-6-luna.toml", entries::luna()),
            ("c-kimi-k3.toml", entries::kimi()),
        ],
        policy: policy(local_only),
        app: Some(mail()),
        hosted: Some(Hosted {
            accounts: vec![FakeAccount {
                id: "openrouter",
                standing: Standing::Granted {
                    to: vec![MAIL],
                    grant: "grant-openrouter",
                    key: Some(KEY),
                },
            }],
            chat: Script {
                chat: vec![Chat::Say(vec!["from the cloud"])],
                dims: 0,
            },
            trust: Trust::ScratchCa,
        }),
        probe: Some(only_ollama(port)),
        ..Plan::default()
    })
    .await
}

async fn rescan(world: &World) {
    InferenceProxy::new(&world.client)
        .await
        .expect("proxy")
        .rescan()
        .await
        .expect("Rescan is answered once the look is done");
}

fn cloud_calls(world: &World) -> usize {
    world.provider.as_ref().expect("provider").requests().len()
}

fn reports(world: &World) -> Vec<(String, String, usize)> {
    world.accountd.as_ref().expect("accountd").calls().reported
}

#[tokio::test(flavor = "multi_thread")]
async fn acceptance_6_with_local_only_on_a_mail_summary_runs_on_the_probed_ollama_and_with_it_stopped_is_unavailable_and_never_goes_to_the_cloud()
 {
    let ollama = fake_ollama(&Bind::Loopback).await;
    let port = port_of(&ollama);
    let world = world(port, LocalOnly::On).await;
    rescan(&world).await;

    // The runtime was found and reported to accountd as an account, with its one model.
    assert_eq!(reports(&world), [("ollama".to_owned(), "ok".to_owned(), 1)]);

    // A Mail-class summary runs on it: the model's own name on the wire, the prompt in the body,
    // on this computer, audited as the account that served it.
    let events = turn(&world, "Summarise: the meeting moved to noon.").await;
    let served = routed(&events);
    assert_eq!(served.account.as_str(), "ollama");
    assert_eq!(served.model.as_str(), "llama3.2-3b");
    assert_eq!(served.locality, Locality::OnDevice);
    let Some(InferEvent::Finished(InferReply::Chat(reply))) = events.last() else {
        panic!("a chat reply, got {events:?}");
    };
    assert_eq!(reply.text, "Two lines.");
    let chats = ollama.chats();
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0]["model"], "llama3.2:3b");
    assert!(chats[0].to_string().contains("the meeting moved to noon"));
    let audited = world.audit.entries();
    assert_eq!((audited.len(), audited[0].account.as_str()), (1, "ollama"));
    assert_eq!(cloud_calls(&world), 0);

    // The fake is stopped: the account goes offline (reported, never deleted) and the same
    // request is `Unavailable`. The cloud account is granted and would answer; it is not asked.
    stop(ollama).await;
    rescan(&world).await;
    assert_eq!(
        reports(&world)[1],
        ("ollama".to_owned(), "offline".to_owned(), 0)
    );
    let events = turn(&world, "Summarise: the meeting moved to noon.").await;
    assert_eq!(
        events,
        vec![InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unavailable
        ))]
    );
    assert_eq!(
        cloud_calls(&world),
        0,
        "local only: no call to a cloud reach"
    );
    assert_eq!(
        world.audit.entries().len(),
        1,
        "the refused turn ran nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cloud_reach_of_acceptance_6_is_live_and_only_local_only_keeps_it_out() {
    // The control of the test above: the same world with `ai.local_only` off and the Ollama
    // stopped is answered by the hosted account, so the zero calls above are the setting's doing.
    let (_reserved, port) = reserved_port();
    let world = world(port, LocalOnly::Off).await;
    rescan(&world).await;
    let events = turn(&world, "Summarise: the meeting moved to noon.").await;
    assert_eq!(routed(&events).account.as_str(), "openrouter");
    assert_eq!(cloud_calls(&world), 1);
    assert!(
        reports(&world).is_empty(),
        "a runtime that was never there is never reported"
    );

    // And with local only turned on in the settings in force, the next request is refused.
    let mut settings: Settings = (*world.served.settings()).clone();
    settings.policy = policy(LocalOnly::On);
    world.served.apply(settings);
    let events = turn(&world, "Summarise again").await;
    assert_eq!(
        events,
        vec![InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unavailable
        ))]
    );
    assert_eq!(cloud_calls(&world), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_that_starts_later_is_found_on_rescan_and_listed_for_the_picker_and_goes_offline_and_back()
 {
    let (_reserved, port) = reserved_port();
    let world = world(port, LocalOnly::On).await;
    rescan(&world).await;
    assert!(world.served.listed().is_empty(), "nothing runs yet");

    let ollama = fake_ollama(&Bind::Port(port)).await;
    rescan(&world).await;
    let listed = world.served.listed();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].card.account.as_str(), "ollama");
    assert_eq!(listed[0].readiness, porter_infer::Readiness::Ready);

    stop(ollama).await;
    rescan(&world).await;
    assert_eq!(
        world.served.listed()[0].readiness,
        porter_infer::Readiness::Unavailable
    );

    let _back = fake_ollama(&Bind::Port(port)).await;
    rescan(&world).await;
    assert_eq!(
        world.served.listed()[0].readiness,
        porter_infer::Readiness::Ready
    );
    let states: Vec<_> = reports(&world)
        .into_iter()
        .map(|(_, state, _)| state)
        .collect();
    assert_eq!(states, ["ok", "offline", "ok"]);
    let events = turn(&world, "Summarise: back again").await;
    assert_eq!(routed(&events).account.as_str(), "ollama");
}

#[tokio::test(flavor = "multi_thread")]
async fn rescan_tells_listeners_the_engines_changed_when_a_runtime_came_up() {
    use zbus::export::futures_core::Stream;
    let (_reserved, port) = reserved_port();
    let world = world(port, LocalOnly::On).await;
    let proxy = InferenceProxy::new(&world.client).await.expect("proxy");
    let mut changed = proxy
        .receive_engines_changed()
        .await
        .expect("signal stream");
    let _ollama = fake_ollama(&Bind::Port(port)).await;
    rescan(&world).await;
    tokio::time::timeout(
        porter_fake::GENEROUS,
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut changed).poll_next(cx)),
    )
    .await
    .expect("EnginesChanged arrives")
    .expect("a signal");
}
