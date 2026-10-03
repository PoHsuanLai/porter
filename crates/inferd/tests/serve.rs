//! The fd session server over scripted seams: the test is the client and the engine. What the
//! machine decides is tested in `session::step_tests`; this proves the loop carries it out:
//! events on the wire in order, engines wanted and released, turns dropped, queued requests
//! started with their own descriptors, and a broken wire ending the session.

mod support;

use inferd::serve::{Seams, TurnStep, serve_session};
use inferd::session::SessionSpec;
use porter_core::capability::{CuaEnv, SpeechMode};
use porter_core::consent::Usage;
use porter_core::need::{CuaNeed, SpeechNeed};
use porter_core::{DataClass, Need, Tier, Tokens};
use porter_infer::{
    AttachIndex, AudioFrame, AudioRate, Base64Bytes, ChatControl, ChatMessage, ChatReply,
    ChatRequest, ClientFrame, CuaBegin, CuaStepReply, ImagePart, ImageSource, InferEvent,
    InferRefusal, InferReply, InferRequest, Knob, MessagePart, ModelError, Reasoning, ReplyShape,
    Role, StopReason, TokenUsage, ToolChoice, ToolParallelism, TranscribeBegin, TranscribeMode,
    TranscribeReply,
};
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use support::*;
use tokio::net::UnixStream;
use tokio::sync::Notify;

struct Rig {
    client: Client,
    runner: Runner,
    audit: Audit,
    released: Arc<Mutex<Vec<porter_infer::ModelRef>>>,
    ended: tokio::task::JoinHandle<()>,
}

fn start(router: FixedRouter, engines: Engines, spec: SessionSpec) -> Rig {
    let (ours, theirs) = UnixStream::pair().expect("pair");
    let runner = Runner::default();
    let audit = Audit::default();
    let released = Arc::clone(&engines.released);
    let seams = Seams {
        router,
        engines,
        runner: runner.clone(),
        audit: audit.clone(),
    };
    let ended = tokio::spawn(async move { serve_session(ours, spec, &seams).await });
    Rig {
        client: Client::new(theirs),
        runner,
        audit,
        released,
        ended,
    }
}

fn usage() -> TokenUsage {
    TokenUsage {
        input: Tokens(1),
        output: Tokens(1),
        cached: Tokens(0),
    }
}

fn answer(text: &str) -> InferReply {
    InferReply::Chat(ChatReply {
        text: text.into(),
        tool_calls: vec![],
        stop: StopReason::EndTurn,
        thought: None,
        usage: usage(),
        served: served(),
    })
}

fn chat_with(parts: Vec<MessagePart>) -> InferRequest {
    InferRequest::Chat(ChatRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            parts,
        }],
        shape: ReplyShape::Text,
        tier: Tier::Fast,
        class: DataClass::Mail,
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

fn chat() -> ClientFrame {
    ClientFrame::Request(chat_with(vec![MessagePart::Text("hi".into())]))
}

fn memfd(content: &[u8]) -> OwnedFd {
    let fd = rustix::fs::memfd_create("t", rustix::fs::MemfdFlags::CLOEXEC).expect("memfd");
    std::io::Write::write_all(
        &mut std::fs::File::from(fd.try_clone().expect("dup")),
        content,
    )
    .expect("fill");
    fd
}

fn image(index: u16) -> MessagePart {
    MessagePart::Image(ImagePart {
        media_type: "image/png".into(),
        source: ImageSource::Attached(AttachIndex(index)),
    })
}

fn mail() -> SessionSpec {
    spec(llm(), DataClass::Mail)
}

async fn started(rig: &Rig, count: usize) {
    let runner = rig.runner.clone();
    eventually("the turns started", || {
        runner.started.lock().expect("lock").len() >= count
    })
    .await;
}

fn push(rig: &Rig, which: usize, step: TurnStep) {
    rig.runner.started.lock().expect("lock")[which]
        .tx
        .send(step)
        .expect("the turn is alive");
}

#[tokio::test]
async fn a_refused_route_is_the_first_event_and_then_the_socket_closes() {
    let mut rig = start(
        FixedRouter(Err(InferRefusal::NeedsGrant)),
        Engines::default(),
        mail(),
    );
    assert_eq!(
        rig.client.event().await,
        Some(InferEvent::Finished(InferReply::Refused(
            InferRefusal::NeedsGrant
        )))
    );
    assert_eq!(rig.client.event().await, None);
    rig.ended.await.expect("the server returned");
    assert!(rig.released.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn a_turn_streams_routed_deltas_and_finished_and_is_audited_once() {
    let mut rig = start(FixedRouter::ready(), Engines::default(), mail());
    rig.client.send(&chat(), &[]).await;
    started(&rig, 1).await;
    push(&rig, 0, TurnStep::Event(InferEvent::TextDelta("he".into())));
    push(
        &rig,
        0,
        TurnStep::Event(InferEvent::TextDelta("llo".into())),
    );
    push(&rig, 0, TurnStep::Done(answer("hello")));
    assert_eq!(
        rig.client.until_finished().await,
        vec![
            InferEvent::Routed(served()),
            InferEvent::TextDelta("he".into()),
            InferEvent::TextDelta("llo".into()),
            InferEvent::Finished(answer("hello")),
        ]
    );
    assert_eq!(*rig.audit.0.lock().expect("lock"), vec![answer("hello")]);

    // The second turn does not say who answers again.
    rig.client.send(&chat(), &[]).await;
    started(&rig, 2).await;
    push(&rig, 1, TurnStep::Done(answer("again")));
    assert_eq!(
        rig.client.until_finished().await,
        vec![InferEvent::Finished(answer("again"))]
    );
}

#[tokio::test]
async fn a_loading_engine_shows_waiting_and_a_request_sent_meanwhile_starts_when_it_is_ready() {
    let go = Arc::new(Notify::new());
    let engines = Engines {
        go: Some(Arc::clone(&go)),
        ..Engines::default()
    };
    let mut rig = start(FixedRouter::loading(), engines, mail());
    assert_eq!(
        rig.client.event().await,
        Some(InferEvent::Waiting(porter_infer::Readiness::Loadable))
    );
    rig.client.send(&chat(), &[]).await;
    // Give the server time to read the frame: nothing may start before the engine is ready.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(rig.runner.started.lock().expect("lock").is_empty());
    go.notify_one();
    started(&rig, 1).await;
    push(&rig, 0, TurnStep::Done(answer("late")));
    assert_eq!(
        rig.client.until_finished().await,
        vec![
            InferEvent::Routed(served()),
            InferEvent::Finished(answer("late"))
        ]
    );
}

#[tokio::test]
async fn an_engine_that_fails_to_start_ends_the_session_with_not_ready() {
    let engines = Engines {
        go: None,
        fail: true,
        ..Engines::default()
    };
    let mut rig = start(FixedRouter::ready(), engines, mail());
    assert_eq!(
        rig.client.event().await,
        Some(InferEvent::Finished(InferReply::Failed(
            ModelError::NotReady
        )))
    );
    assert_eq!(rig.client.event().await, None);
    rig.ended.await.expect("returned");
    assert_eq!(*rig.released.lock().expect("lock"), vec![model()]);
}

#[tokio::test]
async fn one_request_queues_behind_a_running_turn_with_its_own_descriptors() {
    let mut rig = start(FixedRouter::ready(), Engines::default(), mail());
    rig.client.send(&chat(), &[]).await;
    started(&rig, 1).await;

    let queued = chat_with(vec![image(0)]);
    rig.client
        .send(
            &ClientFrame::Request(queued.clone()),
            &[memfd(b"queued picture")],
        )
        .await;
    // A third request while one is queued is refused at once; the queued one stays.
    rig.client.send(&chat(), &[]).await;
    assert_eq!(
        rig.client.event().await,
        Some(InferEvent::Routed(served())),
        "routed went out with the first turn"
    );
    assert_eq!(
        rig.client.event().await,
        Some(InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unsupported
        )))
    );

    push(&rig, 0, TurnStep::Done(answer("first")));
    assert_eq!(
        rig.client.until_finished().await,
        vec![InferEvent::Finished(answer("first"))]
    );
    started(&rig, 2).await;
    {
        let started = rig.runner.started.lock().expect("lock");
        assert_eq!(started[1].request, queued);
        assert_eq!(started[1].attachments, vec![b"queued picture".to_vec()]);
    }
    push(&rig, 1, TurnStep::Done(answer("second")));
    assert_eq!(
        rig.client.until_finished().await,
        vec![InferEvent::Finished(answer("second"))]
    );
}

#[tokio::test]
async fn a_request_that_starts_at_once_gets_its_descriptors() {
    let mut rig = start(FixedRouter::ready(), Engines::default(), mail());
    let request = chat_with(vec![image(1), image(0)]);
    rig.client
        .send(
            &ClientFrame::Request(request),
            &[memfd(b"zero"), memfd(b"one")],
        )
        .await;
    started(&rig, 1).await;
    assert_eq!(
        rig.runner.started.lock().expect("lock")[0].attachments,
        vec![b"zero".to_vec(), b"one".to_vec()]
    );
    push(&rig, 0, TurnStep::Done(answer("ok")));
    rig.client.until_finished().await;
}

#[tokio::test]
async fn a_frame_that_names_more_attachments_than_arrived_ends_the_session() {
    let mut rig = start(FixedRouter::ready(), Engines::default(), mail());
    rig.client
        .send(&ClientFrame::Request(chat_with(vec![image(0)])), &[])
        .await;
    assert_eq!(rig.client.event().await, None);
    rig.ended.await.expect("returned");
    assert!(rig.runner.started.lock().expect("lock").is_empty());
    assert_eq!(
        *rig.released.lock().expect("lock"),
        vec![model()],
        "the engine is given back"
    );
}

#[tokio::test]
async fn cancel_drops_the_turn_audits_and_finishes_cancelled() {
    let mut rig = start(FixedRouter::ready(), Engines::default(), mail());
    rig.client.send(&chat(), &[]).await;
    started(&rig, 1).await;
    let tx = rig.runner.started.lock().expect("lock")[0].tx.clone();
    rig.client.send(&ClientFrame::Cancel, &[]).await;
    assert_eq!(
        rig.client.until_finished().await,
        vec![
            InferEvent::Routed(served()),
            InferEvent::Finished(InferReply::Cancelled)
        ]
    );
    // The turn was dropped: its receiver is gone.
    tx.closed().await;
    assert_eq!(
        *rig.audit.0.lock().expect("lock"),
        vec![InferReply::Cancelled]
    );
}

#[tokio::test]
async fn a_client_that_hangs_up_mid_turn_drops_the_turn_and_gives_the_engine_back() {
    let mut rig = start(FixedRouter::ready(), Engines::default(), mail());
    rig.client.send(&chat(), &[]).await;
    started(&rig, 1).await;
    let tx = rig.runner.started.lock().expect("lock")[0].tx.clone();
    drop(rig.client);
    rig.ended.await.expect("returned");
    assert!(tx.is_closed());
    assert_eq!(*rig.released.lock().expect("lock"), vec![model()]);
}

fn voice() -> SessionSpec {
    spec(
        Need::Speech(SpeechNeed {
            modes: [SpeechMode::Stt].into(),
        }),
        DataClass::Voice,
    )
}

fn pcm(at: u64, samples: usize) -> ClientFrame {
    ClientFrame::Audio(AudioFrame {
        at,
        pcm: Base64Bytes(vec![0; samples * 2]),
    })
}

#[tokio::test]
async fn audio_reaches_the_engine_through_the_machine_and_end_of_audio_too() {
    let mut rig = start(FixedRouter::ready(), Engines::default(), voice());
    rig.client
        .send(
            &ClientFrame::Request(InferRequest::Transcribe(TranscribeBegin {
                mode: TranscribeMode::Streaming,
                lang: porter_infer::LangPick::Auto,
                rate: AudioRate(16_000),
                usage: Usage::Interactive,
            })),
            &[],
        )
        .await;
    started(&rig, 1).await;
    rig.client.send(&pcm(0, 160), &[]).await;
    rig.client.send(&pcm(160, 160), &[]).await;
    // An out-of-order frame would end the turn; these are in order, so both arrive.
    let audio = rig.runner.audio.clone();
    eventually("both frames reached the engine", || {
        audio.lock().expect("lock").len() == 2
    })
    .await;
    rig.client.send(&ClientFrame::EndOfAudio, &[]).await;
    let ended = rig.runner.audio_ended.clone();
    eventually("end of audio reached the engine", || {
        *ended.lock().expect("lock") == 1
    })
    .await;
    let reply = InferReply::Transcribed(TranscribeReply {
        text: "hello".into(),
        audio_ms: 20,
        served: served(),
    });
    push(&rig, 0, TurnStep::Done(reply.clone()));
    assert_eq!(
        rig.client.until_finished().await,
        vec![InferEvent::Routed(served()), InferEvent::Finished(reply)]
    );
}

fn cua() -> SessionSpec {
    spec(
        Need::ComputerUse(CuaNeed {
            environments: [CuaEnv::Desktop].into(),
        }),
        DataClass::Screen,
    )
}

#[tokio::test]
async fn a_computer_use_step_before_a_begin_is_refused_and_after_one_it_runs() {
    let mut rig = start(FixedRouter::ready(), Engines::default(), cua());
    let step = |n: u32| {
        ClientFrame::Request(InferRequest::CuaStep(porter_infer::CuaStepRequest {
            step: porter_infer::StepIndex(n),
            window: porter_infer::WindowGeometry {
                logical: cua_action::Size::new(cua_action::Coord(1), cua_action::Coord(1)),
                scale: cua_action::Scale120(120),
            },
            frame: porter_infer::FrameImage {
                source: ImageSource::Attached(AttachIndex(0)),
                layout: porter_infer::FrameLayout::Encoded(porter_infer::MediaKind::Png),
            },
            cursor: None,
            prev: vec![],
            masked: porter_infer::MaskedRegions(0),
            tree: porter_infer::TreeText::Absent,
            notes: vec![],
        }))
    };
    rig.client.send(&step(0), &[memfd(b"frame 0")]).await;
    assert_eq!(
        rig.client.event().await,
        Some(InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unsupported
        )))
    );

    rig.client
        .send(
            &ClientFrame::Request(InferRequest::CuaBegin(CuaBegin {
                goal: "rename".into(),
                hints: vec![],
                env: CuaEnv::Desktop,
            })),
            &[],
        )
        .await;
    started(&rig, 1).await;
    let ack = InferReply::CuaStep(CuaStepReply {
        thought: None,
        actions: vec![],
        dropped: vec![],
        safety: vec![],
    });
    push(&rig, 0, TurnStep::Done(ack.clone()));
    assert_eq!(
        rig.client.until_finished().await,
        vec![InferEvent::Routed(served()), InferEvent::Finished(ack)]
    );

    rig.client.send(&step(1), &[memfd(b"frame 1")]).await;
    started(&rig, 2).await;
    assert_eq!(
        rig.runner.started.lock().expect("lock")[1].attachments,
        vec![b"frame 1".to_vec()]
    );
}
