//! The pipeline at inferd's edge: the catalogue from what routing lists, the plan, and a two-stage
//! run (a fake `voice_in` model, then a text model answered by the replay engine). No network, no
//! engine process: the replay engine is a task on a socket in the scratch directory.

use super::*;
use crate::replay::cassette::{Cassette, TEST_HEADER};
use crate::replay::engine::serve;
use crate::replay::model::model;
use crate::replay::replayer::Replayer;
use crate::router::Listed;
use crate::runner::{Pin, Pinned, Turns};
use crate::supervise::Supervised;
use crate::testkit::Scratch;
use porter_core::capability::{LanguageSet, LlmCap, LlmFeature, LlmWire, SpeechCap};
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::{AccountId, Billing, Locality, ModelId, Tokens};
use porter_infer::{
    AudioFrame, AudioRate, Base64Bytes, ChatControl, ChatMessage, ChatReply, InferEvent,
    InferRefusal, InferReply, Knob, LangPick, LicenceClass, ModelCard, ModelError, ModelRef,
    Reasoning, ReplyShape, Role, ShowReason, Slot, StageRole, SwapCost, ToolChoice,
    ToolParallelism, TranscribeBegin, TranscribeMode, TranscribeReply,
};
use std::sync::{Arc, Mutex};
use tokio::net::UnixListener;

fn need() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(1000),
    })
}

fn card(account: &str, name: &str, capability: Capability) -> ModelCard {
    ModelCard {
        account: AccountId::parse(account).expect("id"),
        model: ModelId::parse(name).expect("id"),
        locality: Locality::OnDevice,
        billing: Billing::Free,
        capabilities: vec![capability],
    }
}

fn llm_cap(features: &[LlmFeature]) -> Capability {
    Capability::Llm(LlmCap {
        features: features.iter().copied().collect(),
        context: Tokens(8192),
        max_output: Tokens(1024),
        wire: LlmWire::ChatCompletions,
    })
}

fn stt_cap() -> Capability {
    Capability::Speech(SpeechCap {
        modes: [SpeechMode::Stt].into(),
        languages: LanguageSet::Any,
    })
}

fn listed(card: ModelCard) -> Listed {
    Listed::new(
        card,
        porter_infer::Readiness::Ready,
        SwapCost::Resident,
        LicenceClass::Open,
    )
}

fn open_policy() -> Policy {
    Policy {
        local_only: porter_infer::LocalOnly::Off,
        floors: Vec::new(),
    }
}

fn audio_shape(class: DataClass) -> RequestShape {
    RequestShape {
        class,
        inputs: BTreeSet::from([Modality::Audio]),
        answer: Answer::Text,
    }
}

fn plan_for(shape: &RequestShape, models: &[Listed], tiers: &TierMap) -> Result<Pipeline, Refusal> {
    plan(
        shape,
        &need(),
        Tier::Balanced,
        models,
        &open_policy(),
        tiers,
        AutoPolicy::default(),
        DescribeImages::Off,
    )
}

fn roles(pipeline: &Pipeline) -> Vec<(StageRole, String)> {
    pipeline
        .stages
        .iter()
        .map(|s| (s.role, s.picked.chosen.model.as_str().to_owned()))
        .collect()
}

#[test]
fn the_catalogue_puts_each_model_in_the_slots_its_capabilities_say() {
    let models = [
        listed(card("local", "gemma", llm_cap(&[LlmFeature::Chat]))),
        listed(card("local", "whisper", stt_cap())),
        listed(card(
            "local",
            "gemma-ears",
            llm_cap(&[LlmFeature::Chat, LlmFeature::AudioIn, LlmFeature::Vision]),
        )),
    ];
    let catalogue = catalogue_of(&models, &need());
    let slots: Vec<Vec<Slot>> = catalogue
        .iter()
        .map(|one| one.slots.iter().copied().collect())
        .collect();
    assert_eq!(
        slots,
        vec![
            vec![Slot::Text],
            vec![Slot::VoiceIn],
            vec![Slot::Text, Slot::VoiceIn, Slot::ImageIn]
        ]
    );
    assert_eq!(
        catalogue[2].takes,
        BTreeSet::from([Modality::Text, Modality::Image, Modality::Audio])
    );
    assert_eq!(catalogue[1].gives, BTreeSet::from([Modality::Text]));
}

#[test]
fn a_model_that_does_not_meet_the_language_need_is_not_in_the_text_slot() {
    let models = [listed(card("local", "gemma", llm_cap(&[LlmFeature::Chat])))];
    let wants_tools = Need::Llm(LlmNeed {
        features: [LlmFeature::Chat, LlmFeature::Tools].into(),
        context: Tokens(1000),
    });
    let catalogue = catalogue_of(&models, &wants_tools);
    assert!(catalogue[0].slots.is_empty());
}

#[test]
fn audio_with_a_text_only_model_plans_hear_then_answer() {
    let models = [
        listed(card("local", "gemma", llm_cap(&[LlmFeature::Chat]))),
        listed(card("local", "whisper", stt_cap())),
    ];
    let planned = plan_for(
        &audio_shape(DataClass::Prompt),
        &models,
        &TierMap::default(),
    )
    .expect("a plan");
    assert_eq!(
        roles(&planned),
        vec![
            (StageRole::Hear, "whisper".to_owned()),
            (StageRole::Answer, "gemma".to_owned())
        ]
    );
}

#[test]
fn a_planned_stage_carries_the_voice_floor_of_what_it_receives() {
    let models = [
        listed(card("local", "gemma", llm_cap(&[LlmFeature::Chat]))),
        listed(card("local", "whisper", stt_cap())),
    ];
    let planned = plan_for(
        &audio_shape(DataClass::Public),
        &models,
        &TierMap::default(),
    )
    .expect("a plan");
    for stage in &planned.stages {
        assert!(
            stage.receives.0.contains(&DataClass::Voice),
            "{:?} receives voice",
            stage.role
        );
    }
}

#[test]
fn images_and_speech_out_plan_but_are_typed_not_yet_for_the_runner() {
    let models = [
        listed(card("local", "gemma", llm_cap(&[LlmFeature::Chat]))),
        listed(card(
            "local",
            "kokoro",
            Capability::Speech(SpeechCap {
                modes: [SpeechMode::Tts].into(),
                languages: LanguageSet::Any,
            }),
        )),
    ];
    let shape = RequestShape {
        class: DataClass::Notes,
        inputs: BTreeSet::from([Modality::Text]),
        answer: Answer::Speech,
    };
    let planned = plan_for(&shape, &models, &TierMap::default()).expect("a plan");
    assert_eq!(
        roles(&planned),
        vec![
            (StageRole::Answer, "gemma".to_owned()),
            (StageRole::Speak, "kokoro".to_owned())
        ]
    );
    assert!(matches!(unsupported(&planned), Some(Refusal::NotYet(_))));
}

// The two-stage run.

struct FakeEars {
    heard: Mutex<Vec<(String, usize)>>,
}

impl Transcriber for FakeEars {
    async fn transcribe<A: AudioIn, S: ChatSink>(
        &self,
        stage: &porter_infer::Stage,
        _begin: &TranscribeBegin,
        audio: &mut A,
        sink: &mut S,
    ) -> Result<TranscribeReply, ModelError> {
        let mut frames = 0;
        while let crate::speech::AudioPull::Frame(_) = audio.next().await {
            frames += 1;
        }
        self.heard
            .lock()
            .expect("lock")
            .push((stage.picked.chosen.model.as_str().to_owned(), frames));
        let text = "turn the lights off";
        sink.event(InferEvent::Heard(porter_infer::HeardDelta::Final {
            text: text.into(),
            from: 0,
            to: 320,
        }));
        Ok(TranscribeReply {
            text: text.into(),
            audio_ms: 20,
            served: stage.served(),
        })
    }
}

use porter_infer::ChatSink;

#[derive(Default)]
struct Events(Vec<InferEvent>);

impl ChatSink for Events {
    fn event(&mut self, event: InferEvent) -> porter_infer::Flow {
        self.0.push(event);
        porter_infer::Flow::Continue
    }
}

fn chat() -> porter_infer::ChatRequest {
    porter_infer::ChatRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            parts: vec![porter_infer::MessagePart::Text("Please do this:".into())],
        }],
        shape: ReplyShape::Text,
        tier: Tier::Balanced,
        class: DataClass::Prompt,
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

fn pcm() -> AudioFrame {
    AudioFrame {
        at: 0,
        pcm: Base64Bytes(vec![0; 640]),
    }
}

/// The replay text model: answers only a request that carries the words the fake ears heard.
async fn replayed_text_model(scratch: &Scratch) -> (Turns, Listed) {
    let cassette = scratch.path().join("cassette.jsonl");
    let entry = r#"{"when":{"contains":["turn the lights off"]},"reply":{"kind":"text","v":"Lights are off."},"uses":"always"}"#;
    std::fs::write(&cassette, format!("{TEST_HEADER}\n{entry}\n")).expect("cassette");
    let local = model(
        "scripted",
        &cassette,
        model_provider::Tokens(4096),
        scratch.path(),
    )
    .expect("model");
    let replayer = Arc::new(Replayer::new(
        Cassette::parse(&std::fs::read_to_string(&cassette).expect("text")).expect("cassette"),
    ));
    let listener = UnixListener::bind(&local.socket.0).expect("bind");
    tokio::spawn(serve(listener, replayer, None));
    let served = porter_infer::ServedBy {
        account: local.card.account.clone(),
        model: local.card.model.clone(),
        locality: Locality::OnDevice,
    };
    let pin = Pin::new();
    let listed = listed(local.card.clone());
    pin.set(Pinned {
        served,
        model: Some(Arc::new(local)),
        cloud: None,
    });
    (Turns::new(pin, Supervised::idle(), Tier::Balanced), listed)
}

#[tokio::test]
async fn a_voice_request_is_heard_by_one_model_and_answered_by_another_over_the_replay_engine() {
    let scratch = Scratch::new("pipeline-two");
    let (turns, text) = replayed_text_model(&scratch).await;
    let ears = listed(card("local", "whisper", stt_cap()));
    let models = [ears, text];
    let planned = plan_for(
        &audio_shape(DataClass::Prompt),
        &models,
        &TierMap::default(),
    )
    .expect("a plan");
    assert_eq!(
        roles(&planned),
        vec![
            (StageRole::Hear, "whisper".to_owned()),
            (StageRole::Answer, "scripted".to_owned())
        ]
    );
    let input = PipelineInput {
        audio: Some((
            TranscribeBegin {
                mode: TranscribeMode::Batch,
                lang: LangPick::Auto,
                rate: AudioRate(16_000),
                usage: Usage::Interactive,
            },
            VecAudio([pcm(), pcm()].into()),
        )),
        chat: chat(),
    };
    let fake = FakeEars {
        heard: Mutex::default(),
    };
    let mut events = Events::default();
    let reply = run_pipeline(&planned, ShowReason::On, input, &fake, &turns, &mut events).await;

    let InferReply::Chat(ChatReply { text, served, .. }) = reply else {
        panic!("a chat reply, got {reply:?}");
    };
    assert_eq!(text, "Lights are off.", "the answer saw the transcript");
    assert_eq!(served.model.as_str(), "scripted");
    assert_eq!(
        *fake.heard.lock().expect("lock"),
        vec![("whisper".to_owned(), 2)],
        "the hearing stage read both frames"
    );
    // The footer: one Routed, one Stage and (with show_reason on) one Why per stage, in order.
    let shape: Vec<String> = events
        .0
        .iter()
        .filter_map(|event| match event {
            InferEvent::Why(why) => Some(format!("why {why:?}")),
            InferEvent::Routed(served) => Some(format!("routed {}", served.model.as_str())),
            InferEvent::Stage(note) => Some(format!("stage {:?}", note.role)),
            InferEvent::Heard(_) => Some("heard".to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(
        shape,
        [
            "why OnlyOne",
            "routed whisper",
            "stage Hear",
            "heard",
            "why OnlyOne",
            "routed scripted",
            "stage Answer"
        ]
    );
}

#[tokio::test]
async fn show_reason_off_keeps_the_stage_notes_and_drops_the_whys() {
    let scratch = Scratch::new("pipeline-quiet");
    let (turns, text) = replayed_text_model(&scratch).await;
    let models = [listed(card("local", "whisper", stt_cap())), text];
    let planned = plan_for(
        &audio_shape(DataClass::Prompt),
        &models,
        &TierMap::default(),
    )
    .expect("a plan");
    let input = PipelineInput {
        audio: Some((
            TranscribeBegin {
                mode: TranscribeMode::Batch,
                lang: LangPick::Auto,
                rate: AudioRate(16_000),
                usage: Usage::Interactive,
            },
            VecAudio([pcm()].into()),
        )),
        chat: chat(),
    };
    let fake = FakeEars {
        heard: Mutex::default(),
    };
    let mut events = Events::default();
    let _ = run_pipeline(&planned, ShowReason::Off, input, &fake, &turns, &mut events).await;
    assert!(!events.0.iter().any(|e| matches!(e, InferEvent::Why(_))));
    assert_eq!(
        events
            .0
            .iter()
            .filter(|e| matches!(e, InferEvent::Stage(_)))
            .count(),
        2
    );
}

#[tokio::test]
async fn a_plan_with_a_stage_the_runner_does_not_run_is_refused_before_anything_is_announced() {
    let scratch = Scratch::new("pipeline-notyet");
    let (turns, text) = replayed_text_model(&scratch).await;
    let kokoro = listed(card(
        "local",
        "kokoro",
        Capability::Speech(SpeechCap {
            modes: [SpeechMode::Tts].into(),
            languages: LanguageSet::Any,
        }),
    ));
    let shape = RequestShape {
        class: DataClass::Notes,
        inputs: BTreeSet::from([Modality::Text]),
        answer: Answer::Speech,
    };
    let planned = plan_for(&shape, &[text, kokoro], &TierMap::default()).expect("a plan");
    let fake = FakeEars {
        heard: Mutex::default(),
    };
    let mut events = Events::default();
    let input = PipelineInput {
        audio: None,
        chat: chat(),
    };
    let reply = run_pipeline(&planned, ShowReason::On, input, &fake, &turns, &mut events).await;
    assert_eq!(reply, InferReply::Refused(InferRefusal::Unsupported));
    assert!(events.0.is_empty());
}

#[test]
fn a_named_voice_in_model_that_cannot_serve_is_a_refusal_not_a_substitute() {
    let models = [
        listed(card("local", "gemma", llm_cap(&[LlmFeature::Chat]))),
        listed(card("local", "whisper", stt_cap())),
    ];
    let mut tiers = TierMap::default();
    tiers.rows.push(porter_infer::TierRow {
        kind: Slot::VoiceIn,
        tier: Tier::Balanced,
        model: ModelRef {
            account: AccountId::parse("local").expect("id"),
            model: ModelId::parse("ghost").expect("id"),
        },
    });
    match plan_for(&audio_shape(DataClass::Prompt), &models, &tiers) {
        Err(Refusal::Stage { role, refusal }) => {
            assert_eq!(role, StageRole::Hear);
            assert_eq!(
                refusal.declined.map(|d| d.model.model.as_str().to_owned()),
                Some("ghost".to_owned())
            );
        }
        other => panic!("{other:?}"),
    }
}
