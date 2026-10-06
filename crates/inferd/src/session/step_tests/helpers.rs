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

pub(super) fn decided(readiness: Readiness) -> SessionIn {
    SessionIn::Routed(Ok(Routing {
        served: served(),
        readiness,
        why: Why::Named,
        reached: None,
        show: ShowReason::Off,
    }))
}

pub(super) fn decided_why(readiness: Readiness, why: Why, show: ShowReason) -> SessionIn {
    SessionIn::Routed(Ok(Routing {
        served: served(),
        readiness,
        why,
        reached: None,
        show,
    }))
}

pub(super) fn routed() -> SessionOut {
    SessionOut::Emit(InferEvent::Routed(served()))
}

pub(super) fn llm_spec(class: DataClass) -> SessionSpec {
    SessionSpec {
        need: Need::Llm(LlmNeed {
            features: Default::default(),
            context: Tokens(1),
        }),
        class,
        tier: Tier::Balanced,
        usage: porter_core::consent::Usage::Interactive,
    }
}

pub(super) fn speech_spec(class: DataClass) -> SessionSpec {
    SessionSpec {
        need: Need::Speech(SpeechNeed {
            modes: [SpeechMode::Stt, SpeechMode::Tts].into(),
        }),
        class,
        tier: Tier::Fast,
        usage: porter_core::consent::Usage::Interactive,
    }
}

pub(super) fn cua_spec(class: DataClass) -> SessionSpec {
    SessionSpec {
        need: Need::ComputerUse(CuaNeed {
            environments: [CuaEnv::Desktop].into(),
        }),
        class,
        tier: Tier::Best,
        usage: porter_core::consent::Usage::Interactive,
    }
}

pub(super) fn task(class: DataClass) -> InferRequest {
    InferRequest::Task(TaskRequest {
        task: Task::Summarise,
        input: "text".into(),
        class,
        usage: Usage::Interactive,
    })
}

pub(super) fn transcribe() -> InferRequest {
    InferRequest::Transcribe(TranscribeBegin {
        mode: TranscribeMode::Streaming,
        lang: LangPick::Auto,
        rate: AudioRate(16_000),
        usage: Usage::Interactive,
    })
}

pub(super) fn speak(class: DataClass) -> InferRequest {
    InferRequest::Speak(SpeakRequest {
        text: "hi".into(),
        voice: None,
        lang: LanguageTag::parse("en").expect("tag"),
        class,
        usage: Usage::Interactive,
    })
}

pub(super) fn cua_begin() -> InferRequest {
    InferRequest::CuaBegin(CuaBegin {
        goal: "rename".into(),
        hints: vec![],
        env: CuaEnv::Desktop,
    })
}

pub(super) fn audio(at: u64, samples: usize) -> ClientFrame {
    ClientFrame::Audio(AudioFrame {
        at,
        pcm: Base64Bytes(vec![0; samples * 2]),
    })
}

pub(super) fn waiting(queued: Option<InferRequest>) -> Phase {
    Phase::Waiting {
        served: served(),
        queued,
    }
}

pub(super) fn idle(routed: RoutedNote) -> Phase {
    idle_cua(routed, CuaProgress::NotBegun)
}

pub(super) fn idle_cua(routed: RoutedNote, cua: CuaProgress) -> Phase {
    Phase::Idle {
        served: served(),
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
    InferReply::CuaStep(CuaStepReply {
        thought: None,
        actions: vec![],
        dropped: vec![],
        safety: vec![],
    })
}

pub(super) fn cua_step() -> InferRequest {
    InferRequest::CuaStep(porter_infer::CuaStepRequest {
        step: porter_infer::StepIndex(0),
        window: porter_infer::WindowGeometry {
            logical: cua_action::Size::new(cua_action::Coord(1), cua_action::Coord(1)),
            scale: cua_action::Scale120(120),
        },
        frame: porter_infer::FrameImage {
            source: porter_infer::ImageSource::Attached(porter_infer::AttachIndex(0)),
            layout: porter_infer::FrameLayout::Encoded(porter_infer::MediaKind::Png),
        },
        cursor: None,
        prev: vec![],
        masked: porter_infer::MaskedRegions(0),
        tree: porter_infer::TreeText::Absent,
        notes: vec![],
    })
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
