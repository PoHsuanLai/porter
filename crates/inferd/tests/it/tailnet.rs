//! inferd across a Tailscale network, end to end on private buses: two inferds, each in a world of
//! its own (its own bus, scratch directory, fake Tailscale and fake accountd), `pi` that lends
//! and `desk` that calls. Every address is a loopback address of the test's own, so a connection
//! from the one's address to the other's is what the network would carry; nothing leaves the
//! computer, and nothing touches the real Tailscale.

use crate::hosting;

use hosting::accountd::{FakeAccount, Standing};
use hosting::bus::within;
use hosting::engine::{Chat, Script};
use hosting::entries;
use hosting::lab::Lab;
use hosting::rig::{Hosted, Plan, Trust, World};
use hosting::tailnet::{Addresses, Seen, TailnetPlan, addresses, free_port, network};
use inferd::attached::{Attached, Place, Reach};
use inferd::peers::Role;
use inferd::settings::TailnetServe;
use porter_core::{AppId, AppName, DataClass, Isolation, ModelId, Tier};
use porter_dbus::{
    CANDIDATE_KEY_MODELS, CANDIDATE_KEY_NEEDS_APPROVAL, Details, GUEST_KEY_STATE, InferenceProxy,
};
use porter_fake_servers::net::Bind;
use porter_infer::{ComputerName, PlaceId, PlaceState};
use std::net::{IpAddr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpSocket;
use zbus::export::futures_core::Stream;

/// The next item of a stream.
async fn next_of<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

const COMPANION: &str = "org.quire.Companion";

fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

/// `pi` (lends) and `desk` (calls), and what is needed to look at both.
struct Pair {
    pi: World,
    desk: World,
    a: Addresses,
    port: u16,
    _lab: Lab,
}

/// The catalogue both computers share: a model started by inferd (`tiny-chat`) and one served by
/// an engine of the person's own (`ATTACHED`).
fn catalog() -> Vec<(&'static str, String)> {
    vec![
        ("a-attached.toml", entries::attached()),
        ("b-chat.toml", entries::chat()),
        ("c-claude.toml", entries::claude()),
    ]
}

fn policy() -> porter_infer::Policy {
    porter_infer::Policy {
        local_only: porter_infer::LocalOnly::Off,
        floors: Vec::new(),
    }
}

/// `pi` lends (when `serving`) the two models it runs, and sees `desk` as `seen`; `desk` has no
/// engine programs, as a laptop does not, and sees `pi` as one of its own.
async fn pair(name: &str, seen: Seen, serving: bool) -> Pair {
    let a = addresses();
    let port = free_port();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("tn-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch");
    let lab = Lab::start(&Bind::Socket(dir), "lab", &[entries::SERVED], None).await;
    let pi = World::start(Plan {
        catalog: catalog(),
        scripts: vec![(
            "tiny-chat",
            Script {
                chat: vec![Chat::Say(vec!["from pi"])],
                dims: 0,
            },
        )],
        attached: vec![Attached {
            id: ModelId::parse(entries::ATTACHED).expect("id"),
            reach: Reach::Socket(lab.socket()),
            key_file: None,
            place: Place::ThisDevice,
            computer: None,
        }],
        policy: policy(),
        role: Role::Shell,
        app: Some(app("org.quire.Shell")),
        tailnet: Some(TailnetPlan {
            network: network(("nPI", "pi", a.pi), ("nDESK", "desk", a.desk), seen),
            port,
            serving,
        }),
        ..Plan::default()
    })
    .await;
    let desk = World::start(Plan {
        catalog: catalog(),
        engine_programs: false,
        policy: policy(),
        role: Role::Placer,
        app: Some(app(COMPANION)),
        tailnet: Some(TailnetPlan {
            network: network(("nDESK", "desk", a.desk), ("nPI", "pi", a.pi), Seen::Mine),
            port,
            serving: false,
        }),
        ..Plan::default()
    })
    .await;
    Pair {
        pi,
        desk,
        a,
        port,
        _lab: lab,
    }
}

/// Waits until `pi` listens at its address.
async fn listening(world: &World, at: SocketAddr) {
    let mut now = world.tailnet.as_ref().expect("tailnet").listening();
    within("pi to listen", now.wait_for(|now| now.bound.contains(&at)))
        .await
        .expect("listening");
}

async fn proxy(connection: &zbus::Connection) -> InferenceProxy<'_> {
    InferenceProxy::new(connection).await.expect("proxy")
}

/// The name of the error a call came back with.
fn error_name(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        other => format!("{other:?}"),
    }
}

fn computer_error(name: &str) -> String {
    format!("{}{name}", porter_dbus::COMPUTER_ERROR_PREFIX)
}

fn text_of(details: &Details, key: &str) -> Option<String> {
    String::try_from(details.get(key)?.try_clone().ok()?).ok()
}

fn flag_of(details: &Details, key: &str) -> Option<bool> {
    bool::try_from(details.get(key)?.try_clone().ok()?).ok()
}

fn model_ids(details: &Details) -> Vec<String> {
    let models: Vec<(String, String)> = details
        .get(CANDIDATE_KEY_MODELS)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<(String, String)>::try_from(value).ok())
        .unwrap_or_default();
    models.into_iter().map(|(id, _)| id).collect()
}

/// A request sent to `to` from the address `source`, and the whole answer.
async fn http(source: IpAddr, to: SocketAddr, request: &str) -> String {
    let socket = TcpSocket::new_v4().expect("socket");
    socket.bind(SocketAddr::new(source, 0)).expect("bind");
    let mut stream = within("a connection", socket.connect(to))
        .await
        .expect("connect");
    stream.write_all(request.as_bytes()).await.expect("send");
    let mut answer = Vec::new();
    within("the answer", stream.read_to_end(&mut answer))
        .await
        .ok();
    String::from_utf8_lossy(&answer).into_owned()
}

const HELLO: &str = "GET /hello HTTP/1.1\r\nHost: pi\r\n\r\n";

fn chat_body(model: &str) -> String {
    let body = format!(r#"{{"model":"{model}","messages":[{{"role":"user","content":"hi"}}]}}"#);
    format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: pi\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn place(text: &str) -> PlaceId {
    PlaceId::parse(text).expect("place")
}

mod chat {
    use super::*;
    use porter_client::InferSession;
    use porter_core::capability::LlmFeature;
    use porter_core::consent::Usage;
    use porter_core::need::LlmNeed;
    use porter_core::{Need, Tokens};
    use porter_infer::{
        ChatControl, ChatMessage, ChatRequest, ClientFrame, InferEvent, InferRequest, Knob,
        MessagePart, OpenOptions, Reasoning, ReplyShape, Role as ChatRole, ToolChoice,
        ToolParallelism,
    };

    fn llm() -> Need {
        Need::Llm(LlmNeed {
            features: [LlmFeature::Chat].into(),
            context: Tokens(1000),
        })
    }

    fn request() -> InferRequest {
        InferRequest::Chat(ChatRequest {
            messages: vec![ChatMessage {
                role: ChatRole::User,
                parts: vec![MessagePart::Text("hello".into())],
            }],
            shape: ReplyShape::Text,
            tier: Tier::Balanced,
            class: DataClass::Public,
            usage: Usage::Interactive,
            tools: vec![],
            control: ChatControl {
                tool_choice: ToolChoice::Auto,
                tool_calls: ToolParallelism::One,
                max_output: Knob::Off,
                reasoning: Reasoning::EngineDefault,
                sampling: Knob::Off,
                stop: vec![],
                scores: Knob::Off,
            },
        })
    }

    /// What `world` answers to a chat placed at `computer:pi` with `tiny-chat`: the text, or the
    /// words of the refusal.
    pub async fn said(world: &World) -> Result<String, String> {
        let options = OpenOptions::default()
            .with_places(vec![place("computer:pi")])
            .with_place_model(
                place("computer:pi"),
                ModelId::parse("tiny-chat").expect("id"),
            );
        let mut session = match within(
            "Open",
            world
                .accounts
                .session_with(&llm(), DataClass::Public, Tier::Balanced, &options),
        )
        .await
        {
            Ok(session) => session,
            Err(refused) => return Err(format!("{refused:?}")),
        };
        let _ = session.send(ClientFrame::Request(request())).await;
        let mut text = String::new();
        loop {
            match within("the next event", session.next())
                .await
                .expect("event")
            {
                InferEvent::TextDelta(piece) => text.push_str(&piece),
                InferEvent::Finished(reply) => {
                    return match reply {
                        porter_infer::InferReply::Chat(_) => Ok(text),
                        other => Err(format!("{other:?}")),
                    };
                }
                _ => {}
            }
        }
    }
}

/// How `world` sees the place `computer:pi` now (the engines on it are looked at first).
async fn ready(world: &World) -> Option<PlaceState> {
    let places = world.accounts.places().await.expect("places");
    places
        .iter()
        .find(|r| r.id.as_str() == "computer:pi")
        .map(|r| r.state)
}

/// `desk` adds `pi` as Settings does, and answers who is there.
async fn add_pi(pair: &Pair) -> String {
    let (settings, _) = pair
        .desk
        .connect_as(Role::Settings, app("org.quire.Settings"))
        .await;
    let rows = proxy(&settings)
        .await
        .candidates()
        .await
        .expect("candidates");
    assert_eq!(rows.len(), 1, "{rows:?}");
    proxy(&settings)
        .await
        .add_tailnet_computer(&rows[0].0)
        .await
        .expect("added")
}

#[tokio::test(flavor = "multi_thread")]
async fn lending_is_off_by_default_nothing_listens_and_tailscale_is_not_even_asked() {
    let p = pair("off", Seen::Mine, false).await;
    let at = SocketAddr::new(p.a.pi, p.port);
    assert!(
        p.pi.tailnet
            .as_ref()
            .unwrap()
            .listening()
            .borrow()
            .bound
            .is_empty()
    );
    assert!(tokio::net::TcpStream::connect(at).await.is_err());
    assert!(
        p.pi.tailscale.as_ref().unwrap().fake.requests().is_empty(),
        "no question to Tailscale while lending is off: {:?}",
        p.pi.tailscale.as_ref().unwrap().fake.requests()
    );
    // Desk looks and finds nobody lending.
    let (settings, _) = p
        .desk
        .connect_as(Role::Settings, app("org.quire.Settings"))
        .await;
    let rows = proxy(&settings)
        .await
        .candidates()
        .await
        .expect("candidates");
    assert!(rows.is_empty(), "{rows:?}");
    // The setting turns it on, and off.
    let mut on = (*p.pi.served.settings()).clone();
    on.tailnet_serve = TailnetServe::On;
    p.pi.served.apply(on.clone());
    listening(&p.pi, at).await;
    on.tailnet_serve = TailnetServe::Off;
    p.pi.served.apply(on);
    let mut now = p.pi.tailnet.as_ref().unwrap().listening();
    within("pi to stop", now.wait_for(|now| now.bound.is_empty()))
        .await
        .expect("stops");
    // The listener is closed a moment after it is let go of.
    within("the connection to be refused", async {
        while tokio::net::TcpStream::connect(at).await.is_ok() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_computer_is_asked_about_once_and_after_a_yes_its_models_answer() {
    let p = pair("granted", Seen::Mine, true).await;
    listening(&p.pi, SocketAddr::new(p.a.pi, p.port)).await;
    let (pi_shell, _) = (p.pi.client.clone(), ());
    let pi_proxy = proxy(&pi_shell).await;
    let mut asks = pi_proxy.receive_guest_asks().await.expect("signal");

    // Desk finds pi, which says it still needs a yes, and adds it.
    let (settings, _) = p
        .desk
        .connect_as(Role::Settings, app("org.quire.Settings"))
        .await;
    let rows = proxy(&settings)
        .await
        .candidates()
        .await
        .expect("candidates");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "nPI");
    assert_eq!(text_of(&rows[0].1, "name").as_deref(), Some("pi"));
    assert_eq!(
        flag_of(&rows[0].1, CANDIDATE_KEY_NEEDS_APPROVAL),
        Some(true)
    );
    let lent = model_ids(&rows[0].1);
    assert!(lent.contains(&"tiny-chat".to_owned()), "{lent:?}");
    assert!(lent.contains(&entries::ATTACHED.to_owned()), "{lent:?}");
    // The hello did not ask the person anything.
    assert!(pi_proxy.guests().await.expect("guests").is_empty());
    let placed = proxy(&settings)
        .await
        .add_tailnet_computer("nPI")
        .await
        .expect("added");
    assert_eq!(placed, "computer:pi");

    // Not ready: pi turned the first request away and asked its person.
    let places = p.desk.accounts.places().await.expect("places");
    let row = places
        .iter()
        .find(|r| r.id.as_str() == "computer:pi")
        .expect("the place");
    assert_eq!(row.state, PlaceState::NotReady);
    assert_eq!(row.name, "pi");
    let asked = within("the question", next_of(&mut asks))
        .await
        .expect("a signal");
    let args = asked.args().expect("args");
    assert_eq!(args.node, "nDESK");
    assert_eq!(text_of(&args.details, "name").as_deref(), Some("desk"));
    let guests = pi_proxy.guests().await.expect("guests");
    assert_eq!(guests.len(), 1);
    assert_eq!(
        text_of(&guests[0].1, GUEST_KEY_STATE).as_deref(),
        Some("asking")
    );
    // Trying again does not ask again.
    let _ = p.desk.accounts.places().await.expect("places");
    assert_eq!(pi_proxy.guests().await.expect("guests").len(), 1);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), next_of(&mut asks))
            .await
            .is_err(),
        "the person is asked once"
    );

    // The person at pi says yes (the shell may answer a question that is waiting).
    pi_proxy
        .answer_guest("nDESK", true)
        .await
        .expect("answered");
    let places = p.desk.accounts.places().await.expect("places");
    let row = places
        .iter()
        .find(|r| r.id.as_str() == "computer:pi")
        .expect("the place");
    assert_eq!(row.state, PlaceState::Ready);
    let ids: Vec<&str> = row.models.iter().map(|m| m.id.as_str()).collect();
    assert!(
        ids.contains(&"tiny-chat") && ids.contains(&entries::ATTACHED),
        "{ids:?}"
    );

    // The assistant runs on pi: the words come from pi's engine, which saw the prompt.
    assert_eq!(chat::said(&p.desk).await, Ok("from pi".to_owned()));
    let seen = p.pi.engines["tiny-chat"].seen.lock().unwrap().clone();
    let prompts: Vec<String> = seen.iter().map(|s| s.body.to_string()).collect();
    assert!(
        prompts.iter().any(|body| body.contains("hello")),
        "{prompts:?}"
    );
    // And pi's audit trail has the request (tokens, never the words).
    assert!(!p.pi.audit.entries().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_no_keeps_a_computer_out_and_it_is_not_asked_again() {
    let p = pair("denied", Seen::Mine, true).await;
    listening(&p.pi, SocketAddr::new(p.a.pi, p.port)).await;
    let pi_proxy = proxy(&p.pi.client).await;
    let mut asks = pi_proxy.receive_guest_asks().await.expect("signal");
    assert_eq!(add_pi(&p).await, "computer:pi");
    let _ = p.desk.accounts.places().await.expect("places");
    within("the question", next_of(&mut asks))
        .await
        .expect("a signal");
    pi_proxy
        .answer_guest("nDESK", false)
        .await
        .expect("answered");
    for _ in 0..2 {
        let places = p.desk.accounts.places().await.expect("places");
        let row = places
            .iter()
            .find(|r| r.id.as_str() == "computer:pi")
            .unwrap();
        assert_eq!(row.state, PlaceState::NotReady);
    }
    let guests = pi_proxy.guests().await.expect("guests");
    assert_eq!(
        text_of(&guests[0].1, GUEST_KEY_STATE).as_deref(),
        Some("denied")
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), next_of(&mut asks))
            .await
            .is_err(),
        "a computer told no is not asked about again"
    );
    // The assistant cannot be placed there, in words.
    let refused = chat::said(&p.desk).await.expect_err("refused");
    assert!(
        refused.contains("NoAllowedPlace") || refused.contains("NotReady"),
        "{refused}"
    );
    // The words pi gives it.
    let said = http(
        p.a.desk,
        SocketAddr::new(p.a.pi, p.port),
        &chat_body("tiny-chat"),
    )
    .await;
    assert!(said.starts_with("HTTP/1.1 403"), "{said}");
    assert!(
        said.contains("was told not to let this one use its models"),
        "{said}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_yes_can_be_taken_back_and_forgotten_so_the_computer_is_asked_again() {
    let p = pair("revoked", Seen::Mine, true).await;
    listening(&p.pi, SocketAddr::new(p.a.pi, p.port)).await;
    let pi_proxy = proxy(&p.pi.client).await;
    let mut asks = pi_proxy.receive_guest_asks().await.expect("signal");
    add_pi(&p).await;
    let _ = ready(&p.desk).await;
    within("the question", next_of(&mut asks))
        .await
        .expect("a signal");
    pi_proxy.answer_guest("nDESK", true).await.expect("yes");
    assert_eq!(ready(&p.desk).await, Some(PlaceState::Ready));

    // Settings at pi takes it back.
    let (settings, _) =
        p.pi.connect_as(Role::Settings, app("org.quire.Settings"))
            .await;
    let settings = proxy(&settings).await;
    settings.answer_guest("nDESK", false).await.expect("no");
    assert_eq!(ready(&p.desk).await, Some(PlaceState::NotReady));
    // Forgotten, it is a stranger again and is asked again.
    settings.forget_guest("nDESK").await.expect("forgotten");
    assert!(settings.guests().await.expect("guests").is_empty());
    assert_eq!(ready(&p.desk).await, Some(PlaceState::NotReady));
    within("the question again", next_of(&mut asks))
        .await
        .expect("a second signal");
    assert_eq!(settings.guests().await.expect("guests").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn another_user_a_server_and_a_shared_computer_are_refused_unless_the_person_allows_it() {
    for (name, seen) in [
        ("another", Seen::Another),
        ("tagged", Seen::Tagged),
        ("shared", Seen::Shared),
    ] {
        let p = pair(name, seen, true).await;
        let at = SocketAddr::new(p.a.pi, p.port);
        listening(&p.pi, at).await;
        // Never asked about: no question is raised for them, whatever they try.
        let said = http(p.a.desk, at, &chat_body("tiny-chat")).await;
        assert!(said.starts_with("HTTP/1.1 403"), "{name}: {said}");
        let words = match seen {
            Seen::Tagged => "set up as a server",
            _ => "belongs to someone else",
        };
        assert!(said.contains(words), "{name}: {said}");
        let pi_proxy = proxy(&p.pi.client).await;
        assert!(
            pi_proxy.guests().await.expect("guests").is_empty(),
            "{name}"
        );
        // Desk looks and finds nothing to add, and adding it says why in plain words.
        let (settings, _) = p
            .desk
            .connect_as(Role::Settings, app("org.quire.Settings"))
            .await;
        let settings = proxy(&settings).await;
        assert!(
            settings.candidates().await.expect("candidates").is_empty(),
            "{name}"
        );
        let refused = settings
            .add_tailnet_computer("nPI")
            .await
            .expect_err("refused");
        assert_eq!(
            error_name(&refused),
            computer_error("NotAnswering"),
            "{name}"
        );
        // The person's own word, given to Settings at pi ahead of time, lets it in.
        let (pi_settings, _) =
            p.pi.connect_as(Role::Settings, app("org.quire.Settings"))
                .await;
        proxy(&pi_settings)
            .await
            .answer_guest("nDESK", true)
            .await
            .expect("allowed");
        let hello = http(p.a.desk, at, HELLO).await;
        assert!(hello.starts_with("HTTP/1.1 200"), "{name}: {hello}");
        assert!(hello.contains("tiny-chat"), "{name}: {hello}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_from_pis_own_address_is_refused_and_raises_no_question() {
    // A program of another account on pi reaches the listener through the network address and
    // would pass as one of the person's other computers, if nothing looked at where it came from.
    let p = pair("own-address", Seen::Mine, true).await;
    let at = SocketAddr::new(p.a.pi, p.port);
    listening(&p.pi, at).await;
    let said = http(p.a.pi, at, &chat_body("tiny-chat")).await;
    assert!(said.starts_with("HTTP/1.1 403"), "{said}");
    assert!(
        said.contains("won't answer a request that came from itself"),
        "{said}"
    );
    let hello = http(p.a.pi, at, HELLO).await;
    assert!(hello.starts_with("HTTP/1.1 403"), "{hello}");
    assert!(
        proxy(&p.pi.client)
            .await
            .guests()
            .await
            .expect("guests")
            .is_empty()
    );
    // An address Tailscale has no computer for gets the same refusal.
    let stranger = http("127.250.250.250".parse().unwrap(), at, HELLO).await;
    assert!(
        stranger.contains("could not tell which of your computers is asking"),
        "{stranger}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cloud_account_is_never_reached_from_a_peer_even_when_it_is_the_only_engine() {
    // pi has nothing of its own to run and one cloud account that works: it lends nothing.
    let a = addresses();
    let port = free_port();
    let pi = World::start(Plan {
        catalog: vec![("c-claude.toml", entries::claude())],
        policy: policy(),
        role: Role::Shell,
        app: Some(app("org.quire.Shell")),
        hosted: Some(Hosted {
            accounts: vec![FakeAccount {
                id: "openrouter",
                standing: Standing::Granted {
                    to: vec![COMPANION, "org.quire.Guest", "org.quire.Guest.nDESK"],
                    grant: "grant-openrouter",
                    key: Some("sk-or-test-key"),
                },
            }],
            chat: Script {
                chat: vec![Chat::Say(vec!["cloud"])],
                dims: 0,
            },
            trust: Trust::ScratchCa,
        }),
        tailnet: Some(TailnetPlan {
            network: network(("nPI", "pi", a.pi), ("nDESK", "desk", a.desk), Seen::Mine),
            port,
            serving: true,
        }),
        ..Plan::default()
    })
    .await;
    let at = SocketAddr::new(a.pi, port);
    listening(&pi, at).await;
    // The person said yes to desk ahead of time, so only what is lent can stop the request.
    let (settings, _) = pi
        .connect_as(Role::Settings, app("org.quire.Settings"))
        .await;
    proxy(&settings)
        .await
        .answer_guest("nDESK", true)
        .await
        .expect("yes");
    let hello = http(a.desk, at, HELLO).await;
    assert!(hello.contains(r#""models":[]"#), "{hello}");
    let listed = http(a.desk, at, "GET /v1/models HTTP/1.1\r\nHost: pi\r\n\r\n").await;
    assert!(listed.contains(r#""data":[]"#), "{listed}");
    for model in [
        "claude-opus-5.5",
        "gpt-6-luna",
        "openrouter/claude-opus-5.5",
    ] {
        let said = http(a.desk, at, &chat_body(model)).await;
        assert!(said.starts_with("HTTP/1.1 404"), "{model}: {said}");
    }
    assert!(
        pi.provider
            .as_ref()
            .expect("provider")
            .requests()
            .is_empty(),
        "nothing was sent to the account"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_computer_attached_over_the_network_is_never_lent_onward() {
    // pi reaches a machine of the person's through a tunnel (`my-network`); it lends only what
    // runs on pi itself.
    let a = addresses();
    let port = free_port();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("tn-{}-onward", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch");
    let lab = Lab::start(&Bind::Socket(dir), "lab", &[entries::SERVED], None).await;
    let pi = World::start(Plan {
        catalog: vec![("a-attached.toml", entries::attached())],
        attached: vec![Attached {
            id: ModelId::parse(entries::ATTACHED).expect("id"),
            reach: Reach::Socket(lab.socket()),
            key_file: None,
            place: Place::MyNetwork,
            computer: Some(ComputerName::parse("lab").expect("name")),
        }],
        policy: policy(),
        role: Role::Shell,
        app: Some(app("org.quire.Shell")),
        tailnet: Some(TailnetPlan {
            network: network(("nPI", "pi", a.pi), ("nDESK", "desk", a.desk), Seen::Mine),
            port,
            serving: true,
        }),
        ..Plan::default()
    })
    .await;
    let at = SocketAddr::new(a.pi, port);
    listening(&pi, at).await;
    let (settings, _) = pi
        .connect_as(Role::Settings, app("org.quire.Settings"))
        .await;
    proxy(&settings)
        .await
        .answer_guest("nDESK", true)
        .await
        .expect("yes");
    let hello = http(a.desk, at, HELLO).await;
    assert!(hello.contains(r#""models":[]"#), "{hello}");
    let said = http(a.desk, at, &chat_body(entries::ATTACHED)).await;
    assert!(said.starts_with("HTTP/1.1 404"), "{said}");
    assert!(
        lab.chats().is_empty(),
        "nothing was forwarded to the machine behind pi"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_listener_follows_the_address_tailscale_gives_the_computer() {
    let p = pair("address", Seen::Mine, true).await;
    let first = SocketAddr::new(p.a.pi, p.port);
    listening(&p.pi, first).await;
    let moved = SocketAddr::new(p.a.spare, p.port);
    p.pi.tailscale
        .as_ref()
        .unwrap()
        .fake
        .edit(|net| net.me.addresses = vec![p.a.spare.to_string()]);
    listening(&p.pi, moved).await;
    let mut now = p.pi.tailnet.as_ref().unwrap().listening();
    within(
        "the old address to go",
        now.wait_for(|now| !now.bound.contains(&first)),
    )
    .await
    .expect("goes");
    within("the old address to refuse", async {
        while tokio::net::TcpStream::connect(first).await.is_ok() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    // And when Tailscale stops, so does the listener.
    p.pi.tailscale.as_ref().unwrap().fake.stop();
    within(
        "listening to stop",
        now.wait_for(|now| now.bound.is_empty()),
    )
    .await
    .expect("stops");
}

#[tokio::test(flavor = "multi_thread")]
async fn candidates_are_looked_at_only_when_read_at_most_once_a_minute_and_only_with_an_account() {
    let p = pair("looks", Seen::Mine, true).await;
    listening(&p.pi, SocketAddr::new(p.a.pi, p.port)).await;
    let (settings, _) = p
        .desk
        .connect_as(Role::Settings, app("org.quire.Settings"))
        .await;
    let settings = proxy(&settings).await;
    // Nobody read the list: nobody was looked at.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(p.pi.tailscale.as_ref().unwrap().whois_asked(), 0);
    // Read twice, one look.
    assert_eq!(settings.candidates().await.expect("candidates").len(), 1);
    assert_eq!(settings.candidates().await.expect("candidates").len(), 1);
    assert_eq!(p.pi.tailscale.as_ref().unwrap().whois_asked(), 1);

    // With no Tailscale account in the accounts, accountd lists no computers: no look is made.
    let q = pair("no-account", Seen::Mine, true).await;
    listening(&q.pi, SocketAddr::new(q.a.pi, q.port)).await;
    q.desk
        .tailscale
        .as_ref()
        .unwrap()
        .account
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let (settings, _) = q
        .desk
        .connect_as(Role::Settings, app("org.quire.Settings"))
        .await;
    let settings = proxy(&settings).await;
    assert!(settings.candidates().await.expect("candidates").is_empty());
    assert_eq!(q.pi.tailscale.as_ref().unwrap().whois_asked(), 0);
    let refused = settings
        .add_tailnet_computer("nPI")
        .await
        .expect_err("refused");
    assert_eq!(error_name(&refused), computer_error("NotOnTailscale"));
}

#[tokio::test(flavor = "multi_thread")]
async fn adding_a_computer_that_is_not_there_or_twice_is_refused_in_plain_words() {
    let p = pair("adding", Seen::Mine, true).await;
    listening(&p.pi, SocketAddr::new(p.a.pi, p.port)).await;
    let (settings, _) = p
        .desk
        .connect_as(Role::Settings, app("org.quire.Settings"))
        .await;
    let settings = proxy(&settings).await;
    for (node, name) in [("nNOBODY", "NotOnTailscale"), ("nDESK", "NotOnTailscale")] {
        let refused = settings
            .add_tailnet_computer(node)
            .await
            .expect_err("refused");
        assert_eq!(error_name(&refused), computer_error(name), "{node}");
    }
    let invalid = settings
        .add_tailnet_computer("not a node!")
        .await
        .expect_err("refused");
    assert_eq!(
        error_name(&invalid),
        "org.freedesktop.DBus.Error.InvalidArgs"
    );
    assert_eq!(
        settings.add_tailnet_computer("nPI").await.expect("added"),
        "computer:pi"
    );
    // Once added it is no longer offered, and adding it again is refused.
    assert!(settings.candidates().await.expect("candidates").is_empty());
    let twice = settings
        .add_tailnet_computer("nPI")
        .await
        .expect_err("refused");
    assert_eq!(error_name(&twice), computer_error("AlreadyThere"));
    // Removing it takes it away; it is offered again.
    settings
        .remove_computer("computer:pi")
        .await
        .expect("removed");
    let places = p.desk.accounts.places().await.expect("places");
    assert!(places.iter().all(|row| row.id.as_str() != "computer:pi"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_name_follows_tailscale_and_the_place_stays() {
    let p = pair("renamed", Seen::Mine, true).await;
    listening(&p.pi, SocketAddr::new(p.a.pi, p.port)).await;
    add_pi(&p).await;
    p.desk
        .tailscale
        .as_ref()
        .unwrap()
        .fake
        .edit(|net| net.peers[0].name = "kitchen-pi".to_owned());
    let places = p.desk.accounts.places().await.expect("places");
    let row = places
        .iter()
        .find(|r| r.id.as_str() == "computer:pi")
        .expect("same place");
    assert_eq!(row.name, "kitchen-pi");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_settings_may_add_or_look_and_only_settings_and_the_shell_may_answer() {
    let p = pair("callers", Seen::Mine, true).await;
    listening(&p.pi, SocketAddr::new(p.a.pi, p.port)).await;
    let denied = "org.freedesktop.DBus.Error.AccessDenied";
    // On desk: an app, the shell and the companion may not look at or add computers.
    for (role, who) in [
        (Role::App, "org.quire.Memory"),
        (Role::Shell, "org.quire.Shell"),
        (Role::Placer, COMPANION),
    ] {
        let (connection, _) = p.desk.connect_as(role, app(who)).await;
        let inference = proxy(&connection).await;
        let e = inference.candidates().await.expect_err("refused");
        assert_eq!(error_name(&e), denied, "{who}");
        let e = inference
            .add_tailnet_computer("nPI")
            .await
            .expect_err("refused");
        assert_eq!(error_name(&e), denied, "{who}");
        let e = inference.forget_guest("nPI").await.expect_err("refused");
        assert_eq!(error_name(&e), denied, "{who}");
    }
    // On pi: an app and the companion may neither see nor answer the computers that ask.
    for (role, who) in [(Role::App, "org.quire.Memory"), (Role::Placer, COMPANION)] {
        let (connection, _) = p.pi.connect_as(role, app(who)).await;
        let inference = proxy(&connection).await;
        let e = inference.guests().await.expect_err("refused");
        assert_eq!(error_name(&e), denied, "{who}");
        let e = inference
            .answer_guest("nDESK", true)
            .await
            .expect_err("refused");
        assert_eq!(error_name(&e), denied, "{who}");
    }
    // The shell may answer only a computer that is asking.
    let shell = proxy(&p.pi.client).await;
    let e = shell
        .answer_guest("nDESK", true)
        .await
        .expect_err("nobody asked");
    assert_eq!(error_name(&e), computer_error("NotAsking"));
    let e = shell.forget_guest("nDESK").await.expect_err("refused");
    assert_eq!(error_name(&e), denied);
    // Settings at pi may answer one that never asked, if it is on the person's network.
    let (settings, _) =
        p.pi.connect_as(Role::Settings, app("org.quire.Settings"))
            .await;
    let settings = proxy(&settings).await;
    settings.answer_guest("nDESK", true).await.expect("allowed");
    let e = settings
        .answer_guest("nNOBODY", true)
        .await
        .expect_err("not there");
    assert_eq!(error_name(&e), computer_error("NotOnTailscale"));
}
