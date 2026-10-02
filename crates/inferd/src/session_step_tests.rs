//! The rows of models §4.2 and voice §3.4 as one table over `step`, then the invariants that
//! hold for any input.

use super::*;
use porter_core::capability::{CuaEnv, LanguageTag, SpeechMode};
use porter_core::consent::Usage;
use porter_core::need::{CuaNeed, LlmNeed, SpeechNeed};
use porter_core::{AccountId, ModelId, Tokens};
use porter_infer::{
    AudioFrame, Base64Bytes, CuaBegin, LangPick, SpeakRequest, Task, TaskRequest, TranscribeBegin,
    TranscribeMode,
};

fn model() -> ModelRef {
    ModelRef {
        account: AccountId::parse("local").expect("account"),
        model: ModelId::parse("qwen").expect("model"),
    }
}

fn llm_spec(class: DataClass) -> SessionSpec {
    SessionSpec {
        need: Need::Llm(LlmNeed {
            features: Default::default(),
            context: Tokens(1),
        }),
        class,
        tier: Tier::Balanced,
    }
}

fn speech_spec(class: DataClass) -> SessionSpec {
    SessionSpec {
        need: Need::Speech(SpeechNeed {
            modes: [SpeechMode::Stt, SpeechMode::Tts].into(),
        }),
        class,
        tier: Tier::Fast,
    }
}

fn cua_spec(class: DataClass) -> SessionSpec {
    SessionSpec {
        need: Need::ComputerUse(CuaNeed {
            environments: [CuaEnv::Desktop].into(),
        }),
        class,
        tier: Tier::Best,
    }
}

fn task(class: DataClass) -> InferRequest {
    InferRequest::Task(TaskRequest {
        task: Task::Summarise,
        input: "text".into(),
        class,
        usage: Usage::Interactive,
    })
}

fn transcribe() -> InferRequest {
    InferRequest::Transcribe(TranscribeBegin {
        mode: TranscribeMode::Streaming,
        lang: LangPick::Auto,
        rate: AudioRate(16_000),
        usage: Usage::Interactive,
    })
}

fn speak(class: DataClass) -> InferRequest {
    InferRequest::Speak(SpeakRequest {
        text: "hi".into(),
        voice: None,
        lang: LanguageTag::parse("en").expect("tag"),
        class,
        usage: Usage::Interactive,
    })
}

fn cua_begin() -> InferRequest {
    InferRequest::CuaBegin(CuaBegin {
        goal: "rename".into(),
        hints: vec![],
        env: CuaEnv::Desktop,
    })
}

fn audio(at: u64, samples: usize) -> ClientFrame {
    ClientFrame::Audio(AudioFrame {
        at,
        pcm: Base64Bytes(vec![0; samples * 2]),
    })
}

fn waiting(queued: Option<InferRequest>) -> Phase {
    Phase::Waiting {
        chosen: model(),
        queued,
    }
}

fn idle(routed: RoutedNote) -> Phase {
    Phase::Idle {
        chosen: model(),
        routed,
    }
}

fn in_turn(kind: RequestKind, audio: AudioCursor) -> Phase {
    Phase::InTurn {
        chosen: model(),
        kind,
        audio,
    }
}

fn expecting(next: u64) -> AudioCursor {
    AudioCursor::Expecting { next }
}

fn done(reply: InferReply) -> SessionOut {
    SessionOut::Emit(InferEvent::Finished(reply))
}

fn unsupported() -> SessionOut {
    done(InferReply::Refused(InferRefusal::Unsupported))
}

fn frame(f: ClientFrame) -> SessionIn {
    SessionIn::Frame(f)
}

fn request(r: InferRequest) -> SessionIn {
    frame(ClientFrame::Request(r))
}

type Row = (
    &'static str,
    SessionSpec,
    Phase,
    SessionIn,
    Phase,
    Vec<SessionOut>,
);

fn rows() -> Vec<Row> {
    let text = || SessionOut::Emit(InferEvent::TextDelta("hi".into()));
    let failed = |e| done(InferReply::Failed(e));
    let lost = InferReply::Failed(ModelError::Unreachable);
    vec![
        (
            "route refused closes",
            llm_spec(DataClass::Mail),
            Phase::Opened,
            SessionIn::Routed(Err(InferRefusal::NeedsGrant)),
            Phase::Closed,
            vec![done(InferReply::Refused(InferRefusal::NeedsGrant))],
        ),
        (
            "route chosen waits for the engine",
            llm_spec(DataClass::Mail),
            Phase::Opened,
            SessionIn::Routed(Ok(model())),
            waiting(None),
            vec![SessionOut::Want(model())],
        ),
        (
            "engine ready goes idle",
            llm_spec(DataClass::Mail),
            waiting(None),
            SessionIn::EngineReady,
            idle(RoutedNote::Pending),
            vec![],
        ),
        (
            "engine ready starts the queued request",
            llm_spec(DataClass::Mail),
            waiting(Some(task(DataClass::Mail))),
            SessionIn::EngineReady,
            in_turn(RequestKind::Task, AudioCursor::NoAudio),
            vec![SessionOut::StartTurn(task(DataClass::Mail))],
        ),
        (
            "engine ready refuses a queued request that does not fit",
            llm_spec(DataClass::Mail),
            waiting(Some(transcribe())),
            SessionIn::EngineReady,
            idle(RoutedNote::Pending),
            vec![unsupported()],
        ),
        (
            "engine failed while waiting closes",
            llm_spec(DataClass::Mail),
            waiting(None),
            SessionIn::EngineFailed,
            Phase::Closed,
            vec![failed(ModelError::NotReady), SessionOut::Release(model())],
        ),
        (
            "a request while waiting queues",
            llm_spec(DataClass::Mail),
            waiting(None),
            request(task(DataClass::Mail)),
            waiting(Some(task(DataClass::Mail))),
            vec![],
        ),
        (
            "a second request while waiting is refused, the first stays",
            llm_spec(DataClass::Mail),
            waiting(Some(task(DataClass::Mail))),
            request(task(DataClass::Mail)),
            waiting(Some(task(DataClass::Mail))),
            vec![unsupported()],
        ),
        (
            "cancel while waiting drops the queued request",
            llm_spec(DataClass::Mail),
            waiting(Some(task(DataClass::Mail))),
            frame(ClientFrame::Cancel),
            waiting(None),
            vec![done(InferReply::Cancelled)],
        ),
        (
            "a request before the route is refused",
            llm_spec(DataClass::Mail),
            Phase::Opened,
            request(task(DataClass::Mail)),
            Phase::Opened,
            vec![unsupported()],
        ),
        (
            "idle request that fits starts a turn",
            llm_spec(DataClass::Mail),
            idle(RoutedNote::Sent),
            request(task(DataClass::Mail)),
            in_turn(RequestKind::Task, AudioCursor::NoAudio),
            vec![SessionOut::StartTurn(task(DataClass::Mail))],
        ),
        (
            "idle request of the wrong kind is refused",
            llm_spec(DataClass::Mail),
            idle(RoutedNote::Sent),
            request(speak(DataClass::Mail)),
            idle(RoutedNote::Sent),
            vec![unsupported()],
        ),
        (
            "idle request of another class is refused",
            llm_spec(DataClass::Mail),
            idle(RoutedNote::Sent),
            request(task(DataClass::Files)),
            idle(RoutedNote::Sent),
            vec![unsupported()],
        ),
        (
            "computer use on the screen class starts",
            cua_spec(DataClass::Screen),
            idle(RoutedNote::Sent),
            request(cua_begin()),
            in_turn(RequestKind::CuaBegin, AudioCursor::NoAudio),
            vec![SessionOut::StartTurn(cua_begin())],
        ),
        (
            "computer use on another class is refused",
            cua_spec(DataClass::Mail),
            idle(RoutedNote::Sent),
            request(cua_begin()),
            idle(RoutedNote::Sent),
            vec![unsupported()],
        ),
        (
            "transcribe expects audio from sample zero",
            speech_spec(DataClass::Voice),
            idle(RoutedNote::Sent),
            request(transcribe()),
            in_turn(RequestKind::Transcribe, expecting(0)),
            vec![SessionOut::StartTurn(transcribe())],
        ),
        (
            "transcribe on the caller's own class starts",
            speech_spec(DataClass::Files),
            idle(RoutedNote::Sent),
            request(transcribe()),
            in_turn(RequestKind::Transcribe, expecting(0)),
            vec![SessionOut::StartTurn(transcribe())],
        ),
        (
            "speak follows the session class",
            speech_spec(DataClass::Mail),
            idle(RoutedNote::Sent),
            request(speak(DataClass::Mail)),
            in_turn(RequestKind::Speak, AudioCursor::NoAudio),
            vec![SessionOut::StartTurn(speak(DataClass::Mail))],
        ),
        (
            "speak of another class is refused",
            speech_spec(DataClass::Voice),
            idle(RoutedNote::Sent),
            request(speak(DataClass::Mail)),
            idle(RoutedNote::Sent),
            vec![unsupported()],
        ),
        (
            "cancel in idle is nothing",
            llm_spec(DataClass::Mail),
            idle(RoutedNote::Sent),
            frame(ClientFrame::Cancel),
            idle(RoutedNote::Sent),
            vec![],
        ),
        (
            "audio outside a turn is refused",
            speech_spec(DataClass::Voice),
            idle(RoutedNote::Sent),
            frame(audio(0, 8)),
            idle(RoutedNote::Sent),
            vec![unsupported()],
        ),
        (
            "end of audio outside a turn is refused",
            speech_spec(DataClass::Voice),
            idle(RoutedNote::Sent),
            frame(ClientFrame::EndOfAudio),
            idle(RoutedNote::Sent),
            vec![unsupported()],
        ),
        (
            "audio in a non-transcribe turn is refused, the turn runs on",
            speech_spec(DataClass::Voice),
            in_turn(RequestKind::Speak, AudioCursor::NoAudio),
            frame(audio(0, 8)),
            in_turn(RequestKind::Speak, AudioCursor::NoAudio),
            vec![unsupported()],
        ),
        (
            "in-order audio advances the cursor",
            speech_spec(DataClass::Voice),
            in_turn(RequestKind::Transcribe, expecting(0)),
            frame(audio(0, 512)),
            in_turn(RequestKind::Transcribe, expecting(512)),
            vec![],
        ),
        (
            "out-of-order audio fails the turn",
            speech_spec(DataClass::Voice),
            in_turn(RequestKind::Transcribe, expecting(512)),
            frame(audio(0, 8)),
            idle(RoutedNote::Sent),
            vec![
                SessionOut::DropTurn,
                SessionOut::Audit(InferReply::Failed(ModelError::Unreadable)),
                failed(ModelError::Unreadable),
            ],
        ),
        (
            "audio over a second fails the turn",
            speech_spec(DataClass::Voice),
            in_turn(RequestKind::Transcribe, expecting(0)),
            frame(audio(0, 16_001)),
            idle(RoutedNote::Sent),
            vec![
                SessionOut::DropTurn,
                SessionOut::Audit(InferReply::Failed(ModelError::Unreadable)),
                failed(ModelError::Unreadable),
            ],
        ),
        (
            "end of audio stops expecting frames",
            speech_spec(DataClass::Voice),
            in_turn(RequestKind::Transcribe, expecting(512)),
            frame(ClientFrame::EndOfAudio),
            in_turn(RequestKind::Transcribe, AudioCursor::NoAudio),
            vec![],
        ),
        (
            "audio after end of audio fails the turn",
            speech_spec(DataClass::Voice),
            in_turn(RequestKind::Transcribe, AudioCursor::NoAudio),
            frame(audio(512, 8)),
            idle(RoutedNote::Sent),
            vec![
                SessionOut::DropTurn,
                SessionOut::Audit(InferReply::Failed(ModelError::Unreadable)),
                failed(ModelError::Unreadable),
            ],
        ),
        (
            "a model event is passed on",
            llm_spec(DataClass::Mail),
            in_turn(RequestKind::Chat, AudioCursor::NoAudio),
            SessionIn::TurnEvent(InferEvent::TextDelta("hi".into())),
            in_turn(RequestKind::Chat, AudioCursor::NoAudio),
            vec![text()],
        ),
        (
            "a model done audits then finishes",
            llm_spec(DataClass::Mail),
            in_turn(RequestKind::Chat, AudioCursor::NoAudio),
            SessionIn::TurnDone(lost.clone()),
            idle(RoutedNote::Sent),
            vec![SessionOut::Audit(lost.clone()), done(lost)],
        ),
        (
            "cancel in a turn drops the future and still audits",
            llm_spec(DataClass::Mail),
            in_turn(RequestKind::Chat, AudioCursor::NoAudio),
            frame(ClientFrame::Cancel),
            idle(RoutedNote::Sent),
            vec![
                SessionOut::DropTurn,
                SessionOut::Audit(InferReply::Cancelled),
                done(InferReply::Cancelled),
            ],
        ),
        (
            "cancel drops transcribe audio state with the turn",
            speech_spec(DataClass::Voice),
            in_turn(RequestKind::Transcribe, expecting(512)),
            frame(ClientFrame::Cancel),
            idle(RoutedNote::Sent),
            vec![
                SessionOut::DropTurn,
                SessionOut::Audit(InferReply::Cancelled),
                done(InferReply::Cancelled),
            ],
        ),
        (
            "a request during a turn is refused",
            llm_spec(DataClass::Mail),
            in_turn(RequestKind::Chat, AudioCursor::NoAudio),
            request(task(DataClass::Mail)),
            in_turn(RequestKind::Chat, AudioCursor::NoAudio),
            vec![unsupported()],
        ),
        (
            "the engine dying mid-turn closes",
            llm_spec(DataClass::Mail),
            in_turn(RequestKind::Chat, AudioCursor::NoAudio),
            SessionIn::EngineFailed,
            Phase::Closed,
            vec![
                SessionOut::DropTurn,
                failed(ModelError::NotReady),
                SessionOut::Release(model()),
            ],
        ),
        (
            "eof in a turn drops it and releases",
            llm_spec(DataClass::Mail),
            in_turn(RequestKind::Chat, AudioCursor::NoAudio),
            SessionIn::Closed,
            Phase::Closed,
            vec![SessionOut::DropTurn, SessionOut::Release(model())],
        ),
        (
            "eof in idle releases",
            llm_spec(DataClass::Mail),
            idle(RoutedNote::Sent),
            SessionIn::Closed,
            Phase::Closed,
            vec![SessionOut::Release(model())],
        ),
        (
            "eof while waiting releases",
            llm_spec(DataClass::Mail),
            waiting(Some(task(DataClass::Mail))),
            SessionIn::Closed,
            Phase::Closed,
            vec![SessionOut::Release(model())],
        ),
        (
            "eof before the route has nothing to release",
            llm_spec(DataClass::Mail),
            Phase::Opened,
            SessionIn::Closed,
            Phase::Closed,
            vec![],
        ),
        (
            "closed stays closed",
            llm_spec(DataClass::Mail),
            Phase::Closed,
            request(task(DataClass::Mail)),
            Phase::Closed,
            vec![],
        ),
    ]
}

#[test]
fn session_transitions() {
    for (name, spec, phase, input, next, out) in rows() {
        let (got, effects) = step(&spec, phase, input);
        assert_eq!(got, next, "{name}: phase");
        assert_eq!(effects, out, "{name}: effects");
    }
}

#[test]
fn every_turn_ends_with_exactly_one_finished() {
    let spec = speech_spec(DataClass::Voice);
    let inputs = [
        frame(ClientFrame::Cancel),
        SessionIn::TurnDone(InferReply::Cancelled),
        frame(audio(9, 8)),
        SessionIn::EngineFailed,
    ];
    for input in inputs {
        let (_, effects) = step(
            &spec,
            in_turn(RequestKind::Transcribe, expecting(0)),
            input.clone(),
        );
        let finished = effects
            .iter()
            .filter(|e| matches!(e, SessionOut::Emit(InferEvent::Finished(_))))
            .count();
        assert_eq!(finished, 1, "{input:?}");
    }
}

#[test]
fn a_closed_session_never_acts_again() {
    let spec = llm_spec(DataClass::Mail);
    let inputs = [
        SessionIn::EngineReady,
        SessionIn::Routed(Ok(model())),
        frame(ClientFrame::Cancel),
        SessionIn::Closed,
    ];
    for input in inputs {
        assert_eq!(
            step(&spec, Phase::Closed, input.clone()),
            (Phase::Closed, vec![]),
            "{input:?}"
        );
    }
}
