use super::*;
use crate::catalog::parse_entry_text;
use crate::entries;
use crate::local::{EngineConfig, build};
use crate::pipeline::VecAudio;
use crate::speech_host::{FakeSpeechHost, Words};
use crate::testkit::{Scratch, models};
use porter_core::consent::Usage;
use porter_infer::{AudioFrame, AudioRate, Base64Bytes, InferEvent};
use std::path::PathBuf;

fn ears(scratch: &Scratch) -> LocalModel {
    let entry = parse_entry_text(&entries::speech_in()).expect("entry");
    let config = EngineConfig {
        speech_host: Some(PathBuf::from("/nonexistent/speech-host")),
        hf_cache: scratch.path().join("hf"),
        ..EngineConfig::default()
    };
    let sockets = scratch.path().join("s");
    std::fs::create_dir_all(&sockets).expect("sockets");
    build(&[entry], &config, &sockets)
        .pop()
        .expect("a model on the speech host")
}

fn served() -> ServedBy {
    ServedBy {
        account: porter_core::AccountId::parse("local").expect("id"),
        model: porter_core::ModelId::parse("tiny-ears").expect("id"),
        locality: porter_core::Locality::OnDevice,
    }
}

fn begin(mode: TranscribeMode, lang: LangPick) -> TranscribeBegin {
    TranscribeBegin {
        mode,
        lang,
        rate: porter_infer::AudioRate(16_000),
        usage: Usage::Interactive,
    }
}

fn frames(count: usize) -> VecAudio {
    VecAudio(
        (0..count)
            .map(|n| AudioFrame {
                at: (n * 512) as u64,
                pcm: Base64Bytes(vec![0; 1024]),
            })
            .collect(),
    )
}

#[derive(Default)]
struct Events {
    seen: Vec<InferEvent>,
    stop_after: Option<usize>,
}

impl ChatSink for Events {
    fn event(&mut self, event: InferEvent) -> Flow {
        self.seen.push(event);
        match self.stop_after {
            Some(n) if self.seen.len() >= n => Flow::Stop,
            _ => Flow::Continue,
        }
    }
}

fn runner(model: &LocalModel) -> SpeechRunner {
    SpeechRunner::for_model(model, served(), Supervised::idle()).expect("a speech host model")
}

#[tokio::test]
async fn a_streaming_turn_sends_the_audio_and_turns_the_hosts_events_into_heard_deltas() {
    let scratch = Scratch::new("stt-stream");
    let model = ears(&scratch);
    let host = FakeSpeechHost::start(&model.socket.0, Words(vec!["turn", "the", "lights"]));
    let mut events = Events::default();
    let reply = runner(&model)
        .transcribe(
            &begin(TranscribeMode::Streaming, LangPick::Auto),
            &mut frames(3),
            &mut events,
        )
        .await
        .expect("a transcript");
    let partial = |text: &str| {
        InferEvent::Heard(HeardDelta::Partial {
            text: text.into(),
            from: 0,
        })
    };
    assert_eq!(
        events.seen,
        vec![
            partial("turn"),
            partial("turn the"),
            partial("turn the lights"),
            InferEvent::Heard(HeardDelta::Final {
                text: "turn the lights".into(),
                from: 0,
                to: 1536,
            }),
        ]
    );
    assert_eq!(reply.text, "turn the lights");
    assert_eq!(reply.audio_ms, 96);
    assert_eq!(reply.served, served());
    let seen = host.seen.lock().expect("lock");
    assert_eq!(
        (seen.chunks, seen.samples, seen.ends, seen.cancels),
        (3, 1536, 1, 0)
    );
    assert_eq!(seen.requests.len(), 1);
    assert_eq!(seen.requests[0].model.0, "tiny-ears");
    assert_eq!(
        seen.requests[0].mode,
        SttMode::Streaming {
            chunk: AudioMs(560)
        }
    );
    assert_eq!(seen.requests[0].format.rate, SampleRate(16_000));
    assert_eq!(seen.requests[0].format.pcm, PcmFormat::S16Le);
}

#[tokio::test]
async fn a_sink_that_stops_ends_the_turn_early_with_what_was_heard() {
    let scratch = Scratch::new("stt-stop");
    let model = ears(&scratch);
    let _host = FakeSpeechHost::start(&model.socket.0, Words(vec!["one", "two", "three"]));
    let mut events = Events {
        stop_after: Some(1),
        ..Events::default()
    };
    let reply = runner(&model)
        .transcribe(
            &begin(TranscribeMode::Streaming, LangPick::Auto),
            &mut frames(3),
            &mut events,
        )
        .await
        .expect("a transcript");
    assert_eq!(events.seen.len(), 1, "no event after the stop");
    assert_eq!(reply.text, "one two three");
}

#[tokio::test]
async fn no_host_listening_is_unreachable() {
    let scratch = Scratch::new("stt-down");
    let model = ears(&scratch);
    let mut events = Events::default();
    let failed = runner(&model)
        .transcribe(
            &begin(TranscribeMode::Batch, LangPick::Auto),
            &mut frames(1),
            &mut events,
        )
        .await;
    assert_eq!(failed, Err(ModelError::Unreachable));
    assert!(events.seen.is_empty());
}

/// Audio that never ends: the person is still holding the key.
struct Held(mpsc::UnboundedReceiver<AudioFrame>);

impl AudioIn for Held {
    async fn next(&mut self) -> AudioPull {
        match self.0.recv().await {
            Some(frame) => AudioPull::Frame(frame),
            None => std::future::pending().await,
        }
    }
}

#[tokio::test]
async fn dropping_a_turn_in_flight_tells_the_host_to_cancel() {
    let scratch = Scratch::new("stt-cancel");
    let model = ears(&scratch);
    let host = FakeSpeechHost::start(&model.socket.0, Words(vec!["hello"]));
    let (send, receive) = mpsc::unbounded_channel();
    let run = runner(&model);
    let task = tokio::spawn(async move {
        let mut events = Events::default();
        let mut audio = Held(receive);
        run.transcribe(
            &begin(TranscribeMode::Streaming, LangPick::Auto),
            &mut audio,
            &mut events,
        )
        .await
    });
    send.send(AudioFrame {
        at: 0,
        pcm: Base64Bytes(vec![0; 1024]),
    })
    .expect("send");
    let heard_one = async {
        while host.seen.lock().expect("lock").chunks < 1 {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), heard_one)
        .await
        .expect("the host heard a chunk");
    task.abort();
    let cancelled = async {
        while host.seen.lock().expect("lock").cancels < 1 {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), cancelled)
        .await
        .expect("the host was told Cancel");
    assert_eq!(host.seen.lock().expect("lock").ends, 0);
}

#[test]
fn the_request_to_the_host_follows_the_begin_and_the_models_chunk() {
    let scratch = Scratch::new("stt-request");
    let model = ears(&scratch);
    let shape = SttShape::of(&model);
    assert_eq!(shape.model.0, "tiny-ears");
    assert_eq!(shape.chunk, AudioMs(560));
    let tags = |texts: &[&str]| {
        LangPick::Prefer(
            texts
                .iter()
                .map(|t| LanguageTag::parse(t).expect("tag"))
                .collect(),
        )
    };
    let lang = |texts: &[&str]| {
        LangChoice::Prefer(texts.iter().map(|t| Lang::new(*t).expect("lang")).collect())
    };
    let cases = [
        (
            "auto",
            begin(TranscribeMode::Streaming, LangPick::Auto),
            LangChoice::Auto,
            SttMode::Streaming {
                chunk: AudioMs(560),
            },
        ),
        (
            "preferred",
            begin(TranscribeMode::Batch, tags(&["zh-TW", "en"])),
            lang(&["zh-TW", "en"]),
            SttMode::Batch,
        ),
        (
            "none preferred",
            begin(TranscribeMode::Streaming, tags(&[])),
            LangChoice::Auto,
            SttMode::Streaming {
                chunk: AudioMs(560),
            },
        ),
    ];
    for (name, begin, expected_lang, expected_mode) in cases {
        let request = shape.request(&begin);
        assert_eq!(request.lang, expected_lang, "{name}");
        assert_eq!(request.mode, expected_mode, "{name}");
        assert_eq!(request.format.rate, SampleRate(16_000), "{name}");
    }
    let other = SttShape {
        model: ModelName("m".into()),
        chunk: AudioMs(80),
    };
    assert_eq!(
        other
            .request(&begin(TranscribeMode::Streaming, LangPick::Auto))
            .mode,
        SttMode::Streaming { chunk: AudioMs(80) }
    );
    let _ = AudioRate(0);
}

#[test]
fn transcript_events_become_heard_deltas_and_an_unspellable_language_is_dropped() {
    let text = |t: &str| speech_provider::HeardText(t.into());
    let cases = [
        (
            "partial",
            TranscriptEvent::Partial {
                text: text("hi"),
                from: SampleIndex(5),
            },
            Some(HeardDelta::Partial {
                text: "hi".into(),
                from: 5,
            }),
        ),
        (
            "final",
            TranscriptEvent::Final {
                text: text("hi"),
                from: SampleIndex(5),
                to: SampleIndex(9),
            },
            Some(HeardDelta::Final {
                text: "hi".into(),
                from: 5,
                to: 9,
            }),
        ),
        (
            "language",
            TranscriptEvent::Lang(Lang::new("zh-CN").expect("lang")),
            Some(HeardDelta::Lang(LanguageTag::parse("zh-CN").expect("tag"))),
        ),
    ];
    for (name, event, expected) in cases {
        assert_eq!(heard(event), expected, "{name}");
    }
}

#[test]
fn only_a_speech_host_model_makes_a_speech_runner() {
    let scratch = Scratch::new("stt-kinds");
    for model in models(&scratch) {
        assert!(
            SpeechRunner::for_model(&model, served(), Supervised::idle()).is_none(),
            "{}",
            model.entry.id.0
        );
    }
    assert!(SpeechRunner::for_model(&ears(&scratch), served(), Supervised::idle()).is_some());
}
