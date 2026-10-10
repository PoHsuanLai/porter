//! Builders the step tables share.

pub(super) use crate::session::{
    AudioCursor, CuaProgress, Phase, RoutedNote, Routing, SessionIn, SessionOut, SessionSpec, step,
};
use porter_core::capability::{CuaEnv, LanguageTag, SpeechMode};
use porter_core::consent::Usage;
use porter_core::need::{CuaNeed, LlmNeed, SpeechNeed};
use porter_core::{AccountId, DataClass, Locality, ModelId, Need, Tier, Tokens};
use porter_infer::{
    AudioFrame, AudioRate, Base64Bytes, ClientFrame, CuaBegin, CuaStepReply, InferEvent,
    InferRefusal, InferReply, InferRequest, LangPick, ModelError, ModelRef, Readiness, RequestKind,
    ServedBy, ShowReason, SpeakRequest, Task, TaskRequest, TranscribeBegin, TranscribeMode, Why,
};

pub(super) fn model() -> ModelRef {
    ModelRef {
        account: AccountId::parse("local").expect("account"),
        model: ModelId::parse("qwen").expect("model"),
    }
}

pub(super) fn served() -> ServedBy {
    ServedBy {
        account: AccountId::parse("local").expect("account"),
        model: ModelId::parse("qwen").expect("model"),
        locality: Locality::OnDevice,
    }
}

pub(super) fn label() -> porter_infer::ModelLabel {
    porter_infer::ModelLabel("Qwen".into())
}

/// The `Answer` note of a turn on `served()` as routed by `decided`.
pub(super) fn answer() -> porter_infer::StageNote {
    answer_for(Why::Named)
}

/// The `Answer` note of a turn routed for this reason.
pub(super) fn answer_for(why: Why) -> porter_infer::StageNote {
    porter_infer::StageNote {
        role: porter_infer::StageRole::Answer,
        served: served(),
        why,
        name: Some(label()),
    }
}

pub(super) fn answer_event() -> SessionOut {
    SessionOut::Emit(InferEvent::Stage(answer()))
}

pub(super) fn decided(readiness: Readiness) -> SessionIn {
    SessionIn::Routed(Ok(Routing {
        served: served(),
        readiness,
        why: Why::Named,
        reached: None,
        show: ShowReason::Off,
        name: Some(label()),
    }))
}

pub(super) fn decided_why(readiness: Readiness, why: Why, show: ShowReason) -> SessionIn {
    SessionIn::Routed(Ok(Routing {
        served: served(),
        readiness,
        why,
        reached: None,
        show,
        name: Some(label()),
    }))
}

pub(super) fn routed() -> SessionOut {
    SessionOut::Emit(InferEvent::Routed(served()))
}

pub(super) fn llm_spec(class: DataClass) -> SessionSpec {
    SessionSpec {
        need: Need::Llm(LlmNeed::new(Default::default(), Tokens(1))),
        class,
        tier: Tier::Balanced,
        usage: porter_core::consent::Usage::Interactive,
    }
}

pub(super) fn speech_spec(class: DataClass) -> SessionSpec {
    SessionSpec {
        need: Need::Speech(SpeechNeed::new([SpeechMode::Stt, SpeechMode::Tts].into())),
        class,
        tier: Tier::Fast,
        usage: porter_core::consent::Usage::Interactive,
    }
}

pub(super) fn cua_spec(class: DataClass) -> SessionSpec {
    SessionSpec {
        need: Need::ComputerUse(CuaNeed::new([CuaEnv::Desktop].into())),
        class,
        tier: Tier::Best,
        usage: porter_core::consent::Usage::Interactive,
    }
}

pub(super) fn task(class: DataClass) -> InferRequest {
    InferRequest::Task(TaskRequest::new(
        Task::Summarise,
        "text".into(),
        class,
        Usage::Interactive,
    ))
}

pub(super) fn transcribe() -> InferRequest {
    InferRequest::Transcribe(TranscribeBegin::new(
        TranscribeMode::Streaming,
        LangPick::Auto,
        AudioRate(16_000),
        Usage::Interactive,
    ))
}

pub(super) fn speak(class: DataClass) -> InferRequest {
    InferRequest::Speak(SpeakRequest::new(
        "hi".into(),
        LanguageTag::parse("en").expect("tag"),
        class,
        Usage::Interactive,
    ))
}

pub(super) fn cua_begin() -> InferRequest {
    InferRequest::CuaBegin(CuaBegin::new("rename".into(), CuaEnv::Desktop))
}

pub(super) fn audio(at: u64, samples: usize) -> ClientFrame {
    ClientFrame::Audio(AudioFrame {
        at,
        pcm: Base64Bytes(vec![0; samples * 2]),
    })
}

pub(super) fn waiting(queued: Option<InferRequest>) -> Phase {
    waiting_for(answer(), queued)
}

/// Waiting on a model routed with this note.
pub(super) fn waiting_for(answer: porter_infer::StageNote, queued: Option<InferRequest>) -> Phase {
    Phase::Waiting {
        served: served(),
        answer,
        queued,
    }
}

pub(super) fn idle(routed: RoutedNote) -> Phase {
    idle_cua(routed, CuaProgress::NotBegun)
}

pub(super) fn idle_cua(routed: RoutedNote, cua: CuaProgress) -> Phase {
    Phase::Idle {
        served: served(),
        answer: answer(),
        routed,
        cua,
    }
}

pub(super) fn in_turn(kind: RequestKind, audio: AudioCursor) -> Phase {
    in_turn_with(kind, audio, CuaProgress::NotBegun, None)
}

pub(super) fn in_turn_with(
    kind: RequestKind,
    audio: AudioCursor,
    cua: CuaProgress,
    queued: Option<InferRequest>,
) -> Phase {
    Phase::InTurn {
        served: served(),
        answer: answer(),
        kind,
        audio,
        cua,
        queued,
    }
}

pub(super) fn queued_chat(queued: Option<InferRequest>) -> Phase {
    in_turn_with(
        RequestKind::Chat,
        AudioCursor::NoAudio,
        CuaProgress::NotBegun,
        queued,
    )
}

pub(super) fn lost_reply() -> InferReply {
    InferReply::Failed(ModelError::Unreachable)
}

pub(super) fn begun_ack() -> InferReply {
    InferReply::CuaStep(CuaStepReply::new(vec![]))
}

pub(super) fn cua_step() -> InferRequest {
    InferRequest::CuaStep(porter_infer::CuaStepRequest::new(
        porter_infer::StepIndex(0),
        porter_infer::WindowGeometry {
            logical: cua_action::Size::new(cua_action::Coord(1), cua_action::Coord(1)),
            scale: cua_action::Scale120(120),
        },
        porter_infer::FrameImage {
            source: porter_infer::ImageSource::Attached(porter_infer::AttachIndex(0)),
            layout: porter_infer::FrameLayout::Encoded(porter_infer::MediaKind::Png),
        },
        porter_infer::TreeText::Absent,
    ))
}

pub(super) fn expecting(next: u64) -> AudioCursor {
    AudioCursor::Expecting { next }
}

pub(super) fn done(reply: InferReply) -> SessionOut {
    SessionOut::Emit(InferEvent::Finished(reply))
}

pub(super) fn unsupported() -> SessionOut {
    done(InferReply::Refused(InferRefusal::Unsupported))
}

pub(super) fn frame(f: ClientFrame) -> SessionIn {
    SessionIn::Frame(f)
}

pub(super) fn request(r: InferRequest) -> SessionIn {
    frame(ClientFrame::Request(r))
}

pub(super) type Row = (
    &'static str,
    SessionSpec,
    Phase,
    SessionIn,
    Phase,
    Vec<SessionOut>,
);
