//! Transcribe over the speech host, hosted: the real `Inference1` object on a private bus, a
//! client opening a `Transcribe` session through `porter-client`, the real router, supervisor and
//! turn runner, and a fake speech host (the host protocol on a Unix socket) that the supervisor
//! "runs" in place of the `speech-host` binary. No real binary, model, library, microphone or
//! network is touched.

use crate::hosting;

use engine_supervisor::{EngineId, SupervisorConfig};
use hosting::engine::{Chat, Script};
use hosting::entries;
use hosting::rig::{Plan, SpeechPlan, World};
use inferd::pipeline::{PipelineInput, VecAudio, plan, run_pipeline};
use inferd::runner::{Pin, Pinned, Turns};
use inferd::speech::Ears;
use porter_client::{Accounts, ClientError, DbusSession, DbusTransport, InferSession};
use porter_core::capability::SpeechMode;
use porter_core::consent::Usage;
use porter_core::need::SpeechNeed;
use porter_core::{DataClass, Need, Tier};
use porter_infer::{
    Answer, AudioFrame, AudioRate, AutoPolicy, Base64Bytes, ChatMessage, ChatReply, ChatRequest,
    ClientFrame, DescribeImages, Flow, HeardDelta, InferEvent, InferReply, InferRequest, LangPick,
    Modality, ModelError, ModelLabel, ModelRef, OpenOptions, Readiness, RequestShape, Role,
    ShowReason, StageRole, TierMap, TranscribeBegin, TranscribeMode,
};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

const WORDS: [&str; 4] = ["turn", "the", "lights", "off"];

fn stt() -> Need {
    Need::Speech(SpeechNeed::new([SpeechMode::Stt].into()))
}

fn quick() -> SupervisorConfig {
    SupervisorConfig {
        probe_every: Duration::from_millis(20),
        backoff: (Duration::from_millis(50), Duration::from_millis(50)),
        ..SupervisorConfig::default()
    }
}

fn plan_with_ears() -> Plan {
    Plan {
        catalog: vec![("tiny-ears.toml", entries::speech_in())],
        speech: Some(SpeechPlan {
            words: WORDS.to_vec(),
            libs: Some(PathBuf::from("/opt/sherpa/lib")),
        }),
        supervisor: Some(quick()),
        ..Plan::default()
    }
}

fn begin() -> TranscribeBegin {
    TranscribeBegin::new(
        TranscribeMode::Streaming,
        LangPick::Auto,
        AudioRate(16_000),
        Usage::Interactive,
    )
}

fn frame(n: u64) -> AudioFrame {
    AudioFrame {
        at: n * 512,
        pcm: Base64Bytes(vec![0; 1024]),
    }
}

async fn open(world: &World) -> DbusSession {
    world
        .accounts
        .session(&stt(), DataClass::Voice, Tier::Balanced)
        .await
        .expect("a session")
}

/// Sends the request, waits until the turn has started (`Routed`: while the engine loads the
/// session is `Waiting` and refuses audio, which is why voiced buffers it until then), then
/// `frames` frames of audio and the end of audio when `end`. Gives the events seen on the way.
async fn speak(session: &mut DbusSession, frames: u64, end: bool) -> Vec<InferEvent> {
    session
        .send(ClientFrame::Request(InferRequest::Transcribe(begin())))
        .await
        .expect("request");
    let mut before = Vec::new();
    loop {
        let event = tokio::time::timeout(porter_fake::GENEROUS, session.next())
            .await
            .expect("an event in time")
            .expect("an event");
        let started = matches!(event, InferEvent::Routed(_));
        assert!(
            !matches!(event, InferEvent::Finished(_)),
            "the turn ended before it started: {event:?}"
        );
        before.push(event);
        if started {
            break;
        }
    }
    for n in 0..frames {
        session
            .send(ClientFrame::Audio(frame(n)))
            .await
            .expect("audio");
    }
    if end {
        session.send(ClientFrame::EndOfAudio).await.expect("end");
    }
    before
}

async fn until_finished(session: &mut impl InferSession) -> Vec<InferEvent> {
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(porter_fake::GENEROUS, session.next())
            .await
            .expect("an event in time")
            .expect("an event");
        let done = matches!(event, InferEvent::Finished(_));
        events.push(event);
        if done {
            return events;
        }
    }
}

fn heard(events: &[InferEvent]) -> Vec<HeardDelta> {
    events
        .iter()
        .filter_map(|event| match event {
            InferEvent::Heard(delta) => Some(delta.clone()),
            _ => None,
        })
        .collect()
}

fn engine_of(world: &World) -> EngineId {
    world.models[0].spec.id.clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn transcribe_streams_partials_and_a_final_from_the_supervised_speech_host() {
    let world = World::start(plan_with_ears()).await;
    let mut session = open(&world).await;
    let before = speak(&mut session, 4, true).await;
    assert!(
        before.contains(&InferEvent::Waiting(Readiness::Loadable)),
        "a cold engine is said to be loading: {before:?}"
    );
    let events = until_finished(&mut session).await;

    let partial = |text: &str| HeardDelta::Partial {
        text: text.into(),
        from: 0,
    };
    assert_eq!(
        heard(&events),
        vec![
            partial("turn"),
            partial("turn the"),
            partial("turn the lights"),
            partial("turn the lights off"),
            HeardDelta::Final {
                text: "turn the lights off".into(),
                from: 0,
                to: 2048
            },
        ]
    );
    let Some(InferEvent::Finished(InferReply::Transcribed(reply))) = events.last() else {
        panic!("a transcript, got {:?}", events.last());
    };
    assert_eq!(reply.text, "turn the lights off");
    assert_eq!(reply.audio_ms, 128);
    assert_eq!(reply.served.model.as_str(), "tiny-ears");

    // The supervisor started the speech engine from its catalogue entry: the program the
    // configuration names, the weights directory, the socket, and the entry's own arguments;
    // the libraries the host loads are on its path and readable in its sandbox.
    let speech = world.speech.as_ref().expect("speech engines");
    let units = speech.units.lock().expect("lock");
    assert_eq!(units.len(), 1);
    let (id, unit) = &units[0];
    assert_eq!(id, &engine_of(&world));
    assert_eq!(unit.program.0, PathBuf::from("/fake/speech-host"));
    let args: Vec<&str> = unit.args.iter().map(|a| a.0.as_str()).collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| *a == name)
            .and_then(|at| args.get(at + 1).copied())
            .unwrap_or_else(|| panic!("{name} in {args:?}"))
    };
    assert!(flag("--model-dir").contains("models--test--tiny-ears/snapshots/"));
    assert!(flag("--socket").ends_with("speech_host-tiny-ears.sock"));
    assert_eq!((flag("--threads"), flag("--chunk-ms")), ("6", "560"));
    let env: Vec<(&str, &str)> = unit
        .env
        .iter()
        .map(|pair| (pair.name.as_str(), pair.value.as_str()))
        .collect();
    assert!(
        env.contains(&("LD_LIBRARY_PATH", "/opt/sherpa/lib")),
        "{env:?}"
    );
    assert!(
        unit.sandbox
            .read
            .contains(&PathBuf::from("/opt/sherpa/lib"))
    );
    assert_eq!(unit.sandbox.gpu, engine_supervisor::GpuAccess::Absent);

    // The host was asked for this model, as the person said it, at 16 kHz; it kept no audio.
    let seen = speech.seen.lock().expect("lock");
    assert_eq!(seen.requests.len(), 1);
    assert_eq!(seen.requests[0].model.0, "tiny-ears");
    assert_eq!((seen.chunks, seen.samples, seen.ends), (4, 2048, 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_audit_entry_of_a_transcription_counts_audio_and_keeps_no_words() {
    let world = World::start(plan_with_ears()).await;
    let mut session = open(&world).await;
    speak(&mut session, 2, true).await;
    until_finished(&mut session).await;
    drop(session);
    let recorded = async {
        loop {
            let entries = world.audit.entries();
            if !entries.is_empty() {
                return entries;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let entries = tokio::time::timeout(porter_fake::GENEROUS, recorded)
        .await
        .expect("an audit entry");
    assert_eq!(entries.len(), 1);
    let text = format!("{entries:?}");
    assert!(!text.contains("turn the lights"), "{text}");
}

async fn wait_ready(world: &World, spawns: usize) {
    let speech = world.speech.as_ref().expect("speech engines");
    let ready = async {
        loop {
            let up = speech.spawns() >= spawns
                && world.served.readiness(&world.models[0]) == Readiness::Ready;
            if up {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(porter_fake::GENEROUS, ready)
        .await
        .expect("the speech engine is ready");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_engine_restarts_after_a_crash_and_a_turn_in_flight_fails_not_hangs() {
    let world = World::start(plan_with_ears()).await;
    let speech = world.speech.clone().expect("speech engines");

    // A first utterance, so the engine is up.
    let mut first = open(&world).await;
    speak(&mut first, 2, true).await;
    let events = until_finished(&mut first).await;
    assert!(
        matches!(
            events.last(),
            Some(InferEvent::Finished(InferReply::Transcribed(_)))
        ),
        "{events:?}"
    );
    assert_eq!(speech.spawns(), 1);

    // The host dies with the person still talking: the turn ends `Failed`, it does not hang.
    let mut cut = open(&world).await;
    speak(&mut cut, 1, false).await;
    let heard_first = async {
        loop {
            if matches!(cut.next().await, Ok(InferEvent::Heard(_))) {
                return;
            }
        }
    };
    tokio::time::timeout(porter_fake::GENEROUS, heard_first)
        .await
        .expect("heard the first partial");
    speech.crash(&engine_of(&world));
    let _ = cut.send(ClientFrame::Audio(frame(1))).await;
    let events = until_finished(&mut cut).await;
    assert_eq!(
        events.last(),
        Some(&InferEvent::Finished(InferReply::Failed(
            ModelError::Unreachable
        )))
    );

    // The supervisor starts it again, and the next utterance is heard.
    wait_ready(&world, 2).await;
    let mut again = open(&world).await;
    speak(&mut again, 3, true).await;
    let events = until_finished(&mut again).await;
    let Some(InferEvent::Finished(InferReply::Transcribed(reply))) = events.last() else {
        panic!("a transcript after the restart, got {:?}", events.last());
    };
    assert_eq!(reply.text, "turn the lights off");
    assert_eq!(speech.spawns(), 2);
}

async fn ask(accounts: &Accounts<DbusTransport>) -> Result<Readiness, ClientError> {
    accounts
        .prepare(
            &stt(),
            DataClass::Voice,
            Tier::Balanced,
            &OpenOptions::default(),
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn prepare_warms_the_speech_engine_and_reports_its_readiness() {
    let world = World::start(plan_with_ears()).await;
    assert_eq!(
        world.served.readiness(&world.models[0]),
        Readiness::Loadable
    );
    // The first call starts it and says so; the engine then answers `Hello` and is ready.
    assert_eq!(ask(&world.accounts).await, Ok(Readiness::Loading));
    wait_ready(&world, 1).await;
    assert_eq!(ask(&world.accounts).await, Ok(Readiness::Ready));
    assert_eq!(world.speech.as_ref().expect("speech").spawns(), 1);

    // A computer with no speech host configured has nothing to warm.
    let bare = World::start(Plan::default()).await;
    assert_eq!(ask(&bare.accounts).await, Ok(Readiness::Unavailable));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_speech_session_with_no_host_is_refused_not_hung() {
    let world = World::start(Plan::default()).await;
    let mut session = open(&world).await;
    let events = until_finished(&mut session).await;
    assert!(
        matches!(
            events.last(),
            Some(InferEvent::Finished(InferReply::Refused(_)))
        ),
        "{events:?}"
    );
}

fn chat() -> ChatRequest {
    ChatRequest::new(
        vec![ChatMessage {
            role: Role::User,
            parts: vec![porter_infer::MessagePart::Text("Please do this:".into())],
        }],
        Tier::Balanced,
        DataClass::Voice,
        Usage::Interactive,
    )
}

#[derive(Default)]
struct Events(Vec<InferEvent>);

impl porter_infer::ChatSink for Events {
    fn event(&mut self, event: InferEvent) -> Flow {
        self.0.push(event);
        Flow::Continue
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hear_then_answer_pipeline_hears_on_the_speech_host_and_answers_on_a_text_model() {
    let mut world_plan = plan_with_ears();
    world_plan.catalog.push(("tiny-chat.toml", entries::chat()));
    world_plan.scripts = vec![(
        "tiny-chat",
        Script {
            chat: vec![Chat::Say(vec!["Lights ", "are off."])],
            dims: 0,
        },
    )];
    let world = World::start(world_plan).await;

    // The planner: the text model takes no audio, so the audio goes through `voice_in` first.
    let listed = world.served.listed();
    let settings = world.served.settings();
    let shape = RequestShape {
        class: DataClass::Voice,
        inputs: BTreeSet::from([Modality::Audio]),
        answer: Answer::Text,
    };
    let need = Need::Llm(porter_core::need::LlmNeed::new(
        [porter_core::capability::LlmFeature::Chat].into(),
        porter_core::Tokens(1000),
    ));
    let planned = plan(
        &shape,
        &need,
        Tier::Balanced,
        &listed,
        &settings.policy,
        &TierMap::default(),
        AutoPolicy::default(),
        DescribeImages::Off,
    )
    .expect("a plan");
    let roles: Vec<(StageRole, String)> = planned
        .stages
        .iter()
        .map(|s| (s.role, s.picked.chosen.model.as_str().to_owned()))
        .collect();
    assert_eq!(
        roles,
        vec![
            (StageRole::Hear, "tiny-ears".to_owned()),
            (StageRole::Answer, "tiny-chat".to_owned())
        ]
    );

    // The answering model's turn is pinned the way the router would pin it.
    let chat_model = world
        .models
        .iter()
        .find(|m| m.entry.id.0 == "tiny-chat")
        .expect("the chat model");
    let pin = Pin::new();
    pin.set(Pinned {
        served: porter_infer::ServedBy {
            account: chat_model.card.account.clone(),
            model: chat_model.card.model.clone(),
            locality: chat_model.card.locality.clone(),
        },
        model: Some(std::sync::Arc::new(chat_model.clone())),
        cloud: None,
    });
    let turns = Turns::new(pin, world.supervised.clone(), Tier::Balanced);
    let names = world
        .models
        .iter()
        .map(|m| {
            (
                ModelRef {
                    account: m.card.account.clone(),
                    model: m.card.model.clone(),
                },
                ModelLabel(m.entry.label.clone()),
            )
        })
        .collect();
    let input = PipelineInput {
        audio: Some((begin(), VecAudio((0..4).map(frame).collect()))),
        chat: chat(),
        names,
    };
    let mut events = Events::default();
    let reply = run_pipeline(
        &planned,
        ShowReason::On,
        input,
        &Ears::new(world.served.clone()),
        &turns,
        &mut events,
    )
    .await;

    let InferReply::Chat(ChatReply { text, served, .. }) = reply else {
        panic!("a chat reply, got {reply:?}");
    };
    assert_eq!(text, "Lights are off.");
    assert_eq!(served.model.as_str(), "tiny-chat");
    // The speech engine was started for the Hear stage, and heard four frames.
    let speech = world.speech.as_ref().expect("speech engines");
    assert_eq!(speech.spawns(), 1);
    assert_eq!(speech.seen.lock().expect("lock").samples, 2048);
    // The stages were announced in order, the Hear stage by its model's name, and the words
    // arrived as `Heard` events before the answer began.
    let notes: Vec<(StageRole, Option<String>)> = events
        .0
        .iter()
        .filter_map(|event| match event {
            InferEvent::Stage(note) => Some((note.role, note.name.as_ref().map(|n| n.0.clone()))),
            _ => None,
        })
        .collect();
    assert_eq!(
        notes,
        [
            (StageRole::Hear, Some("Tiny ears".to_owned())),
            (StageRole::Answer, Some("Tiny chat".to_owned()))
        ]
    );
    let first_heard = events
        .0
        .iter()
        .position(|e| matches!(e, InferEvent::Heard(_)))
        .expect("heard");
    let first_text = events
        .0
        .iter()
        .position(|e| matches!(e, InferEvent::TextDelta(_)))
        .expect("answer text");
    assert!(first_heard < first_text);
    // The answering model was given the transcript, as words, after the person's own text.
    let bodies = world.engines["tiny-chat"].bodies("/v1/chat/completions");
    assert_eq!(bodies.len(), 1);
    let sent = bodies[0].to_string();
    assert!(sent.contains("turn the lights off"), "{sent}");
}
