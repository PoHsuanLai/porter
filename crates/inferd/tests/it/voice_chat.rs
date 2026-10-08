//! A voice chat on a live session: the real `Inference1` object on a private bus, a client
//! opening a language session through `porter-client` and sending a `Transcribe` request, the
//! audio, `EndOfAudio` and then the chat. The router, the supervisor, the planner, `Ears` over the
//! fake speech host and the text model's fake engine are the real ones; no real binary, model,
//! library, microphone or network is touched.

use crate::hosting;

use engine_supervisor::SupervisorConfig;
use hosting::engine::{Chat, Script};
use hosting::entries;
use hosting::rig::{Plan, SpeechPlan, World};
use porter_client::{DbusSession, InferSession};
use porter_core::capability::LlmFeature;
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::{DataClass, Need, Tier, Tokens};
use porter_infer::{
    AudioFrame, AudioRate, Base64Bytes, ChatControl, ChatMessage, ChatRequest, ClientFrame,
    InferEvent, InferReply, InferRequest, Knob, LangPick, Reasoning, ReplyShape, Role, StageRole,
    ToolChoice, ToolParallelism, TranscribeBegin, TranscribeMode,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

const WORDS: [&str; 4] = ["turn", "the", "lights", "off"];

fn llm() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(1000),
    })
}

fn quick() -> SupervisorConfig {
    SupervisorConfig {
        probe_every: Duration::from_millis(20),
        backoff: (Duration::from_millis(50), Duration::from_millis(50)),
        ..SupervisorConfig::default()
    }
}

/// A text model that does not hear, and a speech model on the fake speech host.
fn plan() -> Plan {
    Plan {
        catalog: vec![
            ("tiny-ears.toml", entries::speech_in()),
            ("tiny-chat.toml", entries::chat()),
        ],
        scripts: vec![(
            "tiny-chat",
            Script {
                chat: vec![Chat::Say(vec!["Lights ", "are off."])],
                dims: 0,
            },
        )],
        speech: Some(SpeechPlan {
            words: WORDS.to_vec(),
            libs: Some(PathBuf::from("/opt/sherpa/lib")),
        }),
        supervisor: Some(quick()),
        ..Plan::default()
    }
}

fn begin() -> TranscribeBegin {
    TranscribeBegin {
        mode: TranscribeMode::Streaming,
        lang: LangPick::Auto,
        rate: AudioRate(16_000),
        usage: Usage::Interactive,
    }
}

fn frame(n: u64) -> AudioFrame {
    AudioFrame {
        at: n * 512,
        pcm: Base64Bytes(vec![0; 1024]),
    }
}

fn chat() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            parts: vec![porter_infer::MessagePart::Text("Please do this:".into())],
        }],
        shape: ReplyShape::Text,
        tier: Tier::Balanced,
        class: DataClass::Voice,
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
    }
}

async fn open(world: &World) -> DbusSession {
    world
        .accounts
        .session(&llm(), DataClass::Voice, Tier::Balanced)
        .await
        .expect("a session")
}

/// The voice chat as a client sends it: no waiting for the engine, the audio at once.
async fn say(session: &mut DbusSession, frames: u64) {
    let sent = [
        ClientFrame::Request(InferRequest::Transcribe(begin())),
        ClientFrame::Audio(frame(0)),
    ];
    for one in sent {
        session.send(one).await.expect("frame");
    }
    for n in 1..frames {
        session
            .send(ClientFrame::Audio(frame(n)))
            .await
            .expect("audio");
    }
    session.send(ClientFrame::EndOfAudio).await.expect("end");
    session
        .send(ClientFrame::Request(InferRequest::Chat(chat())))
        .await
        .expect("chat");
}

async fn next(session: &mut DbusSession) -> InferEvent {
    tokio::time::timeout(Duration::from_secs(20), session.next())
        .await
        .expect("an event in time")
        .expect("an event")
}

async fn until_finished(session: &mut DbusSession) -> Vec<InferEvent> {
    let mut events = Vec::new();
    loop {
        let event = next(session).await;
        let done = matches!(event, InferEvent::Finished(_));
        events.push(event);
        if done {
            return events;
        }
    }
}

/// The events that mark the order of a turn, one word each.
fn shape(events: &[InferEvent]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for event in events {
        let word = match event {
            InferEvent::Routed(_) => "routed".to_owned(),
            InferEvent::Stage(note) => format!(
                "stage {:?} {}",
                note.role,
                note.name.as_ref().map_or("?", |name| name.0.as_str())
            ),
            InferEvent::Heard(_) => "heard".to_owned(),
            InferEvent::TextDelta(_) => "text".to_owned(),
            InferEvent::Finished(InferReply::Chat(_)) => "finished chat".to_owned(),
            InferEvent::Finished(other) => format!("finished {other:?}"),
            _ => continue,
        };
        if out.last() != Some(&word) {
            out.push(word);
        }
    }
    out
}

fn notes(events: &[InferEvent], role: StageRole) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, InferEvent::Stage(note) if note.role == role))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_chat_hears_on_the_speech_host_then_answers_with_one_note_per_stage() {
    let world = World::start(plan()).await;
    let mut session = open(&world).await;
    say(&mut session, 4).await;
    let events = until_finished(&mut session).await;

    // Each stage: its `Routed`, its note (by the model's label), then the stage runs.
    assert_eq!(
        shape(&events),
        [
            "routed",
            "stage Hear Tiny ears",
            "heard",
            "routed",
            "stage Answer Tiny chat",
            "text",
            "finished chat"
        ],
        "{events:?}"
    );
    // One `Answer` note: the session's own is replaced, not doubled.
    assert_eq!(notes(&events, StageRole::Answer), 1);
    assert_eq!(notes(&events, StageRole::Hear), 1);
    let Some(InferEvent::Finished(InferReply::Chat(reply))) = events.last() else {
        panic!("a chat reply, got {:?}", events.last());
    };
    assert_eq!(reply.text, "Lights are off.");
    // The speech engine was started for the Hear stage and heard the four frames; the text
    // model was given the transcript as words.
    let speech = world.speech.as_ref().expect("speech engines");
    assert_eq!(speech.spawns(), 1);
    assert_eq!(speech.seen.lock().expect("lock").samples, 2048);
    let bodies = world.engines["tiny-chat"].bodies("/v1/chat/completions");
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].to_string().contains("turn the lights off"));
    // A plain text chat on the same session is the one-stage path it always was.
    session
        .send(ClientFrame::Request(InferRequest::Chat(chat())))
        .await
        .expect("chat");
    let again = until_finished(&mut session).await;
    assert_eq!(notes(&again, StageRole::Answer), 1);
    assert_eq!(notes(&again, StageRole::Hear), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_text_model_that_hears_stays_one_stage() {
    let mut hearing = plan();
    hearing.hears = vec!["tiny-chat"];
    let world = World::start(hearing).await;
    let mut session = open(&world).await;
    say(&mut session, 2).await;
    let events = until_finished(&mut session).await;
    assert_eq!(
        shape(&events),
        ["routed", "stage Answer Tiny chat", "text", "finished chat"],
        "{events:?}"
    );
    assert_eq!(notes(&events, StageRole::Answer), 1);
    // The speech engine was never started.
    assert_eq!(world.speech.as_ref().expect("speech").spawns(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hear_stage_whose_host_is_down_ends_the_turn_not_hangs_it() {
    let world = World::start(plan()).await;
    let speech = world.speech.clone().expect("speech engines");
    speech.refuse.store(true, Ordering::SeqCst);
    let mut session = open(&world).await;
    say(&mut session, 2).await;
    let events = until_finished(&mut session).await;
    // The Hear stage was announced, failed, and the answering model was never asked.
    assert_eq!(notes(&events, StageRole::Hear), 1, "{events:?}");
    assert_eq!(notes(&events, StageRole::Answer), 0, "{events:?}");
    assert!(
        matches!(
            events.last(),
            Some(InferEvent::Finished(InferReply::Failed(_)))
        ),
        "{events:?}"
    );
    assert!(
        world.engines["tiny-chat"]
            .bodies("/v1/chat/completions")
            .is_empty()
    );
    // The session is still usable: the next plain chat is answered.
    session
        .send(ClientFrame::Request(InferRequest::Chat(chat())))
        .await
        .expect("chat");
    let again = until_finished(&mut session).await;
    assert!(matches!(
        again.last(),
        Some(InferEvent::Finished(InferReply::Chat(_)))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_chat_with_no_speech_model_is_refused() {
    let mut bare = plan();
    bare.catalog.remove(0);
    bare.speech = None;
    let world = World::start(bare).await;
    let mut session = open(&world).await;
    say(&mut session, 2).await;
    let events = until_finished(&mut session).await;
    assert!(
        matches!(
            events.last(),
            Some(InferEvent::Finished(InferReply::Refused(_)))
        ),
        "{events:?}"
    );
    assert_eq!(notes(&events, StageRole::Answer), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_during_hear_stops_both_stages() {
    let world = World::start(plan()).await;
    let speech = world.speech.clone().expect("speech engines");
    // The host takes the audio and does not answer its end: the Hear stage is mid-way.
    speech.hold_end.store(true, Ordering::SeqCst);
    let mut session = open(&world).await;
    say(&mut session, 3).await;
    let mut seen = Vec::new();
    loop {
        let event = next(&mut session).await;
        let hear = matches!(&event, InferEvent::Stage(note) if note.role == StageRole::Hear);
        seen.push(event);
        if hear {
            break;
        }
    }
    let waited = async {
        while speech.seen.lock().expect("lock").ends == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(20), waited)
        .await
        .expect("the host was given the whole utterance");

    session.send(ClientFrame::Cancel).await.expect("cancel");
    let rest = until_finished(&mut session).await;
    assert_eq!(
        rest.last(),
        Some(&InferEvent::Finished(InferReply::Cancelled)),
        "{rest:?}"
    );
    // Let the host go: nothing is heard and the answering stage never starts.
    speech.hold_end.store(false, Ordering::SeqCst);
    let after = tokio::time::timeout(Duration::from_millis(400), session.next()).await;
    assert!(after.is_err(), "no more events after the cancel: {after:?}");
    seen.extend(rest);
    assert_eq!(notes(&seen, StageRole::Answer), 0, "{seen:?}");
    assert!(
        world.engines["tiny-chat"]
            .bodies("/v1/chat/completions")
            .is_empty()
    );
}
