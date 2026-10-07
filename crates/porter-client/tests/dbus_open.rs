//! `DbusTransport::open_with` against a fake inferd on a private bus: the need, class, tier and
//! trace context cross as `Inference1.Open` arguments; the returned fd carries wire frames both
//! ways, memfds ride as SCM_RIGHTS, and every way the daemon can fail shows as an error.
#![cfg(all(feature = "dbus", feature = "infer"))]

mod common;

use common::bus::PrivateBus;
use common::inferd::{Behaviour, FakeInferd, Seen, slugs};
use porter_client::{
    Accounts, ClientError, DbusSession, DbusTransport, InferSession, SessionError, TransportError,
};
use porter_core::consent::Usage;
use porter_core::need::{DimsNeed, EmbedNeed, LlmNeed};
use porter_core::{AccountId, DataClass, Dims, ModelId, Need, Tier, Tokens};
use porter_fake::{Script, ScriptStep};
use porter_infer::{
    AttachIndex, ChatControl, ChatMessage, ChatReply, ChatRequest, ClientFrame, EmbedReply,
    EmbedRequest, EmbedRole, EmbedVector, ImagePart, ImageSource, InferEvent, InferRefusal,
    InferReply, InferRequest, Knob, MessagePart, OpenOptions, Reasoning, ReplyShape, RequestKind,
    Role, ServedBy, StopReason, TokenUsage, ToolChoice, ToolParallelism, Traceparent,
};
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn llm() -> Need {
    Need::Llm(LlmNeed {
        features: Default::default(),
        context: Tokens(8_000),
    })
}

fn embeddings() -> Need {
    Need::Embeddings(EmbedNeed {
        dims: DimsNeed::Exactly(Dims(3)),
        modalities: [porter_core::capability::Modality::Text].into(),
    })
}

fn served() -> ServedBy {
    ServedBy {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse("echo").expect("id"),
        locality: porter_core::Locality::OnDevice,
    }
}

fn usage() -> TokenUsage {
    TokenUsage {
        input: Tokens(2),
        output: Tokens(1),
        cached: Tokens(0),
    }
}

fn chat_with(parts: Vec<MessagePart>) -> InferRequest {
    InferRequest::Chat(ChatRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            parts,
        }],
        shape: ReplyShape::Text,
        tier: Tier::Fast,
        class: DataClass::Public,
        usage: Usage::Interactive,
        tools: vec![],
        control: ChatControl {
            tool_choice: ToolChoice::Auto,
            tool_calls: ToolParallelism::Many,
            max_output: Knob::Off,
            reasoning: Reasoning::EngineDefault,
            sampling: Knob::Off,
            stop: vec![],
        },
    })
}

fn chat() -> InferRequest {
    chat_with(vec![MessagePart::Text("hello".into())])
}

fn chat_reply(text: &str) -> InferReply {
    InferReply::Chat(ChatReply {
        text: text.into(),
        tool_calls: vec![],
        stop: StopReason::EndTurn,
        thought: None,
        usage: usage(),
        served: served(),
    })
}

fn chat_script(text: &str) -> Script {
    Script {
        kind: RequestKind::Chat,
        steps: vec![
            ScriptStep::Emit(InferEvent::Routed(served())),
            ScriptStep::Emit(InferEvent::TextDelta(text.into())),
            ScriptStep::Emit(InferEvent::Finished(chat_reply(text))),
        ],
    }
}

struct Rig {
    // Held so the daemon outlives the test body.
    _bus: PrivateBus,
    _daemon: zbus::Connection,
    seen: Arc<Mutex<Seen>>,
    accounts: Accounts<DbusTransport>,
}

async fn rig(behaviour: Behaviour) -> Rig {
    let bus = PrivateBus::start();
    let (fake, seen) = FakeInferd::new(behaviour);
    let daemon = bus.connect().await;
    fake.serve(&daemon).await;
    let accounts = Accounts::over(DbusTransport::over(bus.connect().await));
    Rig {
        _bus: bus,
        _daemon: daemon,
        seen,
        accounts,
    }
}

async fn session(rig: &Rig, need: &Need, class: DataClass) -> DbusSession {
    rig.accounts
        .session(need, class, Tier::Fast)
        .await
        .expect("open")
}

#[tokio::test(flavor = "multi_thread")]
async fn open_carries_the_need_class_and_tier_and_a_chat_turn_streams() {
    let rig = rig(Behaviour::Scripted(vec![chat_script("hi")])).await;
    let mut session = session(&rig, &llm(), DataClass::Public).await;
    session
        .send(ClientFrame::Request(chat()))
        .await
        .expect("send");
    let mut events = Vec::new();
    loop {
        let event = session.next().await.expect("event");
        let done = matches!(event, InferEvent::Finished(_));
        events.push(event);
        if done {
            break;
        }
    }
    assert_eq!(
        events,
        vec![
            InferEvent::Routed(served()),
            InferEvent::TextDelta("hi".into()),
            InferEvent::Finished(chat_reply("hi")),
        ]
    );
    let seen = rig.seen.lock().expect("lock");
    let (class, tier) = slugs(DataClass::Public, Tier::Fast);
    assert_eq!(seen.opens.len(), 1);
    assert_eq!(seen.opens[0].need, llm());
    assert_eq!((&seen.opens[0].class, &seen.opens[0].tier), (&class, &tier));
    assert_eq!(seen.opens[0].traceparent, None);
    assert_eq!(seen.frames, vec![ClientFrame::Request(chat())]);
}

#[tokio::test(flavor = "multi_thread")]
async fn infer_runs_an_embedding_to_its_reply_as_memoryd_does() {
    let reply = InferReply::Embed(EmbedReply {
        vectors: vec![EmbedVector(vec![0.5, 0.25, 0.125])],
        usage: usage(),
        served: served(),
    });
    let rig = rig(Behaviour::Scripted(vec![Script {
        kind: RequestKind::Embed,
        steps: vec![ScriptStep::Emit(InferEvent::Finished(reply.clone()))],
    }]))
    .await;
    let request = InferRequest::Embed(EmbedRequest {
        inputs: vec!["a note".into()],
        role: EmbedRole::Document,
        dims: DimsNeed::Exactly(Dims(3)),
        class: DataClass::Notes,
        usage: Usage::Background,
    });
    let got = rig
        .accounts
        .infer(&embeddings(), DataClass::Notes, Tier::Fast, request)
        .await;
    assert_eq!(got, Ok(reply));
    let seen = rig.seen.lock().expect("lock");
    assert_eq!(seen.opens[0].need, embeddings());
    assert_eq!(seen.opens[0].class, "notes");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_traceparent_rides_in_the_options() {
    let rig = rig(Behaviour::Scripted(vec![])).await;
    let text = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let options = OpenOptions {
        traceparent: Some(Traceparent::parse(text).expect("traceparent")),
        ..OpenOptions::default()
    };
    rig.accounts
        .session_with(&llm(), DataClass::Public, Tier::Best, &options)
        .await
        .expect("open");
    let seen = rig.seen.lock().expect("lock");
    assert_eq!(seen.opens[0].traceparent.as_deref(), Some(text));
    assert_eq!(seen.opens[0].tier, "best");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_usage_rides_in_the_options_as_its_slug_and_is_absent_otherwise() {
    let rig = rig(Behaviour::Scripted(vec![])).await;
    let options = OpenOptions::default().with_usage(porter_core::consent::Usage::Background);
    for options in [&options, &OpenOptions::default()] {
        rig.accounts
            .session_with(&llm(), DataClass::Public, Tier::Fast, options)
            .await
            .expect("open");
    }
    let seen = rig.seen.lock().expect("lock");
    assert_eq!(seen.opens[0].usage.as_deref(), Some("background"));
    assert_eq!(seen.opens[1].usage, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_is_the_first_event() {
    let rig = rig(Behaviour::Refuse(InferRefusal::NeedsGrant)).await;
    let mut session = session(&rig, &llm(), DataClass::Mail).await;
    assert_eq!(
        session.next().await,
        Ok(InferEvent::Finished(InferReply::Refused(
            InferRefusal::NeedsGrant
        )))
    );
    let got = rig
        .accounts
        .infer(&llm(), DataClass::Mail, Tier::Fast, chat())
        .await;
    assert_eq!(
        got,
        Err(ClientError::InferRefused(InferRefusal::NeedsGrant))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_memfd_rides_with_the_frame_that_names_it() {
    let rig = rig(Behaviour::Scripted(vec![chat_script("seen")])).await;
    let mut session = session(&rig, &llm(), DataClass::Public).await;
    let image = std::fs::File::from(
        rustix::fs::memfd_create("frame", rustix::fs::MemfdFlags::CLOEXEC).expect("memfd"),
    );
    std::io::Write::write_all(&mut &image, b"not really a png").expect("fill the memfd");
    let request = chat_with(vec![
        MessagePart::Text("what is this".into()),
        MessagePart::Image(ImagePart {
            media_type: "image/png".into(),
            source: ImageSource::Attached(AttachIndex(0)),
        }),
    ]);
    session
        .send_with(
            ClientFrame::Request(request.clone()),
            &[OwnedFd::from(image)],
        )
        .await
        .expect("send");
    while !matches!(session.next().await, Ok(InferEvent::Finished(_))) {}
    let seen = rig.seen.lock().expect("lock");
    assert_eq!(seen.frames, vec![ClientFrame::Request(request)]);
    assert_eq!(seen.attachments, vec![b"not really a png".to_vec()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_attachment_count_that_is_not_what_the_frame_names_is_refused_unwritten() {
    let rig = rig(Behaviour::Scripted(vec![])).await;
    let mut session = session(&rig, &llm(), DataClass::Public).await;
    let fds: Vec<OwnedFd> = (0..=porter_client::MAX_ATTACHMENTS)
        .map(|_| rustix::fs::memfd_create("x", rustix::fs::MemfdFlags::CLOEXEC).expect("memfd"))
        .collect();
    let got = session.send_with(ClientFrame::Cancel, &fds).await;
    assert!(matches!(got, Err(SessionError::Malformed(_))), "{got:?}");
    let named = chat_with(vec![MessagePart::Image(ImagePart {
        media_type: "image/png".into(),
        source: ImageSource::Attached(AttachIndex(0)),
    })]);
    let none = session.send_with(ClientFrame::Request(named), &[]).await;
    assert!(matches!(none, Err(SessionError::Malformed(_))), "{none:?}");
    // Nothing was written, so the session still works.
    session.send(ClientFrame::Cancel).await.expect("send");
    session.send(ClientFrame::Cancel).await.expect("send");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.seen.lock().expect("lock").frames,
        vec![ClientFrame::Cancel, ClientFrame::Cancel]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_frame_that_arrives_a_byte_at_a_time_is_assembled() {
    let rig = rig(Behaviour::Dribble(vec![chat_script("slow")])).await;
    let mut session = session(&rig, &llm(), DataClass::Public).await;
    session
        .send(ClientFrame::Request(chat()))
        .await
        .expect("send");
    assert_eq!(session.next().await, Ok(InferEvent::Routed(served())));
    assert_eq!(
        session.next().await,
        Ok(InferEvent::TextDelta("slow".into()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn next_is_cancel_safe_mid_frame() {
    let rig = rig(Behaviour::Dribble(vec![chat_script("x")])).await;
    let mut session = session(&rig, &llm(), DataClass::Public).await;
    session
        .send(ClientFrame::Request(chat()))
        .await
        .expect("send");
    // Drop `next` over and over before it can finish a frame; no byte may be lost.
    let mut events = Vec::new();
    while events.len() < 3 {
        if let Ok(event) = tokio::time::timeout(Duration::from_micros(50), session.next()).await {
            events.push(event.expect("event"));
        }
    }
    assert_eq!(events[0], InferEvent::Routed(served()));
    assert_eq!(events[1], InferEvent::TextDelta("x".into()));
    assert_eq!(events[2], InferEvent::Finished(chat_reply("x")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_hangs_up_closes_the_session() {
    let rig = rig(Behaviour::HangUp).await;
    let mut session = session(&rig, &llm(), DataClass::Public).await;
    session
        .send(ClientFrame::Request(chat()))
        .await
        .expect("the write lands before the hang-up");
    assert_eq!(session.next().await, Err(SessionError::Closed));
    assert_eq!(
        session.next().await,
        Err(SessionError::Closed),
        "and stays closed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn bytes_that_are_not_a_frame_are_malformed() {
    let junk = [&4_u32.to_be_bytes()[..], b"nope"].concat();
    let rig = rig(Behaviour::Raw(junk)).await;
    let mut session = session(&rig, &llm(), DataClass::Public).await;
    assert!(matches!(
        session.next().await,
        Err(SessionError::Malformed(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_length_beyond_the_limit_is_malformed_not_buffered() {
    let rig = rig(Behaviour::Raw(u32::MAX.to_be_bytes().to_vec())).await;
    let mut session = session(&rig, &llm(), DataClass::Public).await;
    assert!(matches!(
        session.next().await,
        Err(SessionError::Malformed(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn no_daemon_on_the_bus_is_unreachable() {
    let bus = PrivateBus::start();
    let accounts = Accounts::over(DbusTransport::over(bus.connect().await));
    let got = accounts
        .session(&llm(), DataClass::Public, Tier::Fast)
        .await
        .map(|_| ());
    assert_eq!(
        got,
        Err(ClientError::Transport(TransportError::Unreachable))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_errors_on_open_is_not_unreachable() {
    // The frozen skeleton answers every method NotSupported: the daemon is there but does not
    // speak this call, which is not "unreachable".
    let bus = PrivateBus::start();
    let daemon = bus.connect().await;
    daemon
        .object_server()
        .at(porter_dbus::INFERENCE_PATH, porter_dbus::InferenceSkeleton)
        .await
        .expect("serve");
    daemon
        .request_name(porter_dbus::INFERENCE_BUS)
        .await
        .expect("name");
    let accounts = Accounts::over(DbusTransport::over(bus.connect().await));
    let got = accounts
        .session(&llm(), DataClass::Public, Tier::Fast)
        .await
        .map(|_| ());
    assert!(
        matches!(
            got,
            Err(ClientError::Transport(TransportError::Malformed(_)))
        ),
        "{got:?}"
    );
}
