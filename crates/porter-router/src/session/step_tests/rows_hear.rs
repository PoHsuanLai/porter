//! The rows of a voice chat on a language session: a `Transcribe` request, the audio buffered,
//! `EndOfAudio`, then the chat, which starts a pipeline turn.

use super::helpers::*;
use crate::session::{EngineNow, HeardAudio, MAX_HEARD_MS};
use porter_core::DataClass;
use porter_core::consent::Usage;
use porter_infer::{
    AudioFrame, Base64Bytes, ChatMessage, ChatRequest, ClientFrame, InferEvent, InferReply,
    InferRequest, ModelError, RequestKind, Role, TranscribeBegin,
};

fn chat() -> InferRequest {
    InferRequest::Chat(ChatRequest::new(
        vec![ChatMessage {
            role: Role::User,
            parts: vec![porter_infer::MessagePart::Text("Please do this:".into())],
        }],
        porter_core::Tier::Balanced,
        DataClass::Voice,
        Usage::Interactive,
    ))
}

fn begin() -> TranscribeBegin {
    match transcribe() {
        InferRequest::Transcribe(begin) => begin,
        other => panic!("not a transcribe: {other:?}"),
    }
}

fn frame_of(at: u64, samples: usize) -> AudioFrame {
    AudioFrame {
        at,
        pcm: Base64Bytes(vec![0; samples * 2]),
    }
}

fn hearing_with(
    frames: Vec<AudioFrame>,
    audio: AudioCursor,
    engine: EngineNow,
    chat: Option<InferRequest>,
) -> Phase {
    Phase::Hearing {
        served: served(),
        answer: answer(),
        routed: RoutedNote::Pending,
        cua: CuaProgress::NotBegun,
        heard: HeardAudio {
            begin: begin(),
            frames,
        },
        audio,
        engine,
        chat,
    }
}

fn spec() -> SessionSpec {
    llm_spec(DataClass::Voice)
}

#[test]
fn a_transcribe_on_a_language_session_opens_a_voice_chat_and_announces_nothing() {
    let idle = idle(RoutedNote::Pending);
    let (phase, out) = step(&spec(), idle, request(transcribe()));
    assert_eq!(
        phase,
        hearing_with(vec![], expecting(0), EngineNow::Ready, None)
    );
    assert_eq!(out, vec![]);
}

#[test]
fn it_opens_while_the_engine_loads_and_buffers_the_audio_meanwhile() {
    let (phase, out) = step(&spec(), waiting(None), request(transcribe()));
    assert_eq!(
        phase,
        hearing_with(vec![], expecting(0), EngineNow::Loading, None)
    );
    assert_eq!(out, vec![]);
    let (phase, out) = step(&spec(), phase, frame(audio(0, 8)));
    assert_eq!(
        phase,
        hearing_with(vec![frame_of(0, 8)], expecting(8), EngineNow::Loading, None)
    );
    assert_eq!(out, vec![]);
}

#[test]
fn a_transcribe_behind_a_queued_request_is_refused_as_ever() {
    let (_, out) = step(
        &spec(),
        waiting(Some(task(DataClass::Voice))),
        request(transcribe()),
    );
    assert_eq!(out, vec![unsupported()]);
}

#[test]
fn another_rate_is_refused_and_the_session_stays_as_it_was() {
    let mut other = begin();
    other.rate = porter_infer::AudioRate(48_000);
    let (phase, out) = step(
        &spec(),
        idle(RoutedNote::Pending),
        request(InferRequest::Transcribe(other)),
    );
    assert_eq!(phase, idle(RoutedNote::Pending));
    assert_eq!(out, vec![unsupported()]);
}

#[test]
fn the_chat_after_the_audio_starts_a_pipeline_turn_with_no_note_of_its_own() {
    let phase = hearing_with(
        vec![frame_of(0, 8)],
        AudioCursor::NoAudio,
        EngineNow::Ready,
        None,
    );
    let (phase, out) = step(&spec(), phase, request(chat()));
    assert_eq!(phase, in_turn(RequestKind::Chat, AudioCursor::NoAudio));
    // The stages announce themselves: no `Routed` and no `Answer` note from the machine.
    assert_eq!(
        out,
        vec![
            SessionOut::Hear(HeardAudio {
                begin: begin(),
                frames: vec![frame_of(0, 8)]
            }),
            SessionOut::StartTurn(chat())
        ]
    );
    assert!(!out.iter().any(|o| matches!(o, SessionOut::Emit(_))));
}

#[test]
fn a_chat_before_the_audio_ends_is_refused_and_changes_nothing() {
    let phase = hearing_with(vec![frame_of(0, 8)], expecting(8), EngineNow::Ready, None);
    let (after, out) = step(&spec(), phase.clone(), request(chat()));
    assert_eq!(after, phase);
    assert_eq!(out, vec![unsupported()]);
}

#[test]
fn a_chat_that_arrives_while_the_engine_loads_waits_for_it_and_then_starts() {
    let phase = hearing_with(
        vec![frame_of(0, 8)],
        AudioCursor::NoAudio,
        EngineNow::Loading,
        None,
    );
    let (phase, out) = step(&spec(), phase, request(chat()));
    assert_eq!(out, vec![]);
    assert_eq!(
        phase,
        hearing_with(
            vec![frame_of(0, 8)],
            AudioCursor::NoAudio,
            EngineNow::Loading,
            Some(chat())
        )
    );
    let (phase, out) = step(&spec(), phase, SessionIn::EngineReady);
    assert_eq!(phase, in_turn(RequestKind::Chat, AudioCursor::NoAudio));
    assert!(matches!(out.last(), Some(SessionOut::StartTurn(_))));
}

#[test]
fn end_of_audio_closes_it_and_a_second_one_is_refused() {
    let phase = hearing_with(vec![frame_of(0, 8)], expecting(8), EngineNow::Ready, None);
    let (phase, out) = step(&spec(), phase, frame(ClientFrame::EndOfAudio));
    assert_eq!(
        phase,
        hearing_with(
            vec![frame_of(0, 8)],
            AudioCursor::NoAudio,
            EngineNow::Ready,
            None
        )
    );
    assert_eq!(out, vec![]);
    let (_, out) = step(&spec(), phase, frame(ClientFrame::EndOfAudio));
    assert_eq!(out, vec![unsupported()]);
}

#[test]
fn an_utterance_with_no_audio_fails_unreadable() {
    let phase = hearing_with(vec![], expecting(0), EngineNow::Ready, None);
    let (phase, out) = step(&spec(), phase, frame(ClientFrame::EndOfAudio));
    assert_eq!(phase, idle(RoutedNote::Pending));
    assert_eq!(out, vec![done(InferReply::Failed(ModelError::Unreadable))]);
}

#[test]
fn audio_out_of_order_fails_the_voice_chat_and_drops_what_was_kept() {
    let phase = hearing_with(vec![frame_of(0, 8)], expecting(8), EngineNow::Ready, None);
    let (phase, out) = step(&spec(), phase, frame(audio(99, 8)));
    assert_eq!(phase, idle(RoutedNote::Pending));
    assert_eq!(out, vec![done(InferReply::Failed(ModelError::Unreadable))]);
}

#[test]
fn the_buffer_is_bounded() {
    // One second a frame at 16 kHz: the bound is `MAX_HEARD_MS` of them.
    let seconds = u64::from(MAX_HEARD_MS / 1_000);
    let mut phase = step(&spec(), idle(RoutedNote::Pending), request(transcribe())).0;
    for n in 0..seconds {
        let (next, out) = step(&spec(), phase, frame(audio(n * 16_000, 16_000)));
        assert_eq!(out, vec![], "second {n}");
        phase = next;
    }
    let (phase, out) = step(&spec(), phase, frame(audio(seconds * 16_000, 16_000)));
    assert_eq!(phase, idle(RoutedNote::Pending));
    assert_eq!(out, vec![done(InferReply::Failed(ModelError::Unreadable))]);
}

#[test]
fn cancel_drops_the_audio_and_says_so() {
    let phase = hearing_with(vec![frame_of(0, 8)], expecting(8), EngineNow::Loading, None);
    let (phase, out) = step(&spec(), phase, frame(ClientFrame::Cancel));
    assert_eq!(phase, waiting(None));
    assert_eq!(out, vec![done(InferReply::Cancelled)]);
}

#[test]
fn closing_releases_the_engine_and_there_is_no_turn_to_drop() {
    let phase = hearing_with(vec![], expecting(0), EngineNow::Ready, None);
    let (phase, out) = step(&spec(), phase, SessionIn::Closed);
    assert_eq!(phase, Phase::Closed);
    assert_eq!(out, vec![SessionOut::Release(model())]);
}

#[test]
fn an_engine_that_fails_to_start_ends_the_session_with_a_failure() {
    let phase = hearing_with(vec![], expecting(0), EngineNow::Loading, None);
    let (phase, out) = step(&spec(), phase, SessionIn::EngineFailed);
    assert_eq!(phase, Phase::Closed);
    assert_eq!(
        out,
        vec![
            done(InferReply::Failed(ModelError::NotReady)),
            SessionOut::Release(model())
        ]
    );
}

#[test]
fn a_speech_session_still_takes_a_transcribe_as_a_turn() {
    let (phase, out) = step(
        &speech_spec(DataClass::Voice),
        idle(RoutedNote::Sent),
        request(transcribe()),
    );
    assert_eq!(phase, in_turn(RequestKind::Transcribe, expecting(0)));
    assert_eq!(out, vec![SessionOut::StartTurn(transcribe())]);
    let _ = InferEvent::Waiting;
}
