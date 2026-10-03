//! The rows of the one request that may queue behind a running turn.

#![allow(unused_imports)]
use super::helpers::*;
use porter_core::consent::Usage;
use porter_core::{DataClass, Permille};
use porter_infer::{
    ClientFrame, InferEvent, InferRefusal, InferReply, ModelError, Readiness, RequestKind,
};

pub(super) fn rows() -> Vec<Row> {
    let failed = |e| done(InferReply::Failed(e));
    vec![
        (
            "a request during a turn queues",
            llm_spec(DataClass::Mail),
            in_turn(RequestKind::Chat, AudioCursor::NoAudio),
            request(task(DataClass::Mail)),
            queued_chat(Some(task(DataClass::Mail))),
            vec![],
        ),
        (
            "a second request during a turn is refused, the first stays queued",
            llm_spec(DataClass::Mail),
            queued_chat(Some(task(DataClass::Mail))),
            request(task(DataClass::Mail)),
            queued_chat(Some(task(DataClass::Mail))),
            vec![unsupported()],
        ),
        (
            "a request that does not fit is refused at once, not queued",
            llm_spec(DataClass::Mail),
            in_turn(RequestKind::Chat, AudioCursor::NoAudio),
            request(speak(DataClass::Mail)),
            in_turn(RequestKind::Chat, AudioCursor::NoAudio),
            vec![unsupported()],
        ),
        (
            "the turn ending starts the queued request",
            llm_spec(DataClass::Mail),
            queued_chat(Some(task(DataClass::Mail))),
            SessionIn::TurnDone(lost_reply()),
            in_turn(RequestKind::Task, AudioCursor::NoAudio),
            vec![
                SessionOut::Audit(lost_reply()),
                done(lost_reply()),
                SessionOut::StartTurn(task(DataClass::Mail)),
            ],
        ),
        (
            "cancel ends the running turn and the queued request then starts",
            llm_spec(DataClass::Mail),
            queued_chat(Some(task(DataClass::Mail))),
            frame(ClientFrame::Cancel),
            in_turn(RequestKind::Task, AudioCursor::NoAudio),
            vec![
                SessionOut::DropTurn,
                SessionOut::Audit(InferReply::Cancelled),
                done(InferReply::Cancelled),
                SessionOut::StartTurn(task(DataClass::Mail)),
            ],
        ),
        (
            "a failed audio turn lets the queued request start",
            speech_spec(DataClass::Voice),
            in_turn_with(
                RequestKind::Transcribe,
                expecting(512),
                CuaProgress::NotBegun,
                Some(speak(DataClass::Voice)),
            ),
            frame(audio(0, 8)),
            in_turn(RequestKind::Speak, AudioCursor::NoAudio),
            vec![
                SessionOut::DropTurn,
                SessionOut::Audit(InferReply::Failed(ModelError::Unreadable)),
                done(InferReply::Failed(ModelError::Unreadable)),
                SessionOut::StartTurn(speak(DataClass::Voice)),
            ],
        ),
        (
            "the engine dying tells the queued request too",
            llm_spec(DataClass::Mail),
            queued_chat(Some(task(DataClass::Mail))),
            SessionIn::EngineFailed,
            Phase::Closed,
            vec![
                SessionOut::DropTurn,
                failed(ModelError::NotReady),
                failed(ModelError::NotReady),
                SessionOut::Release(model()),
            ],
        ),
        (
            "eof with a queued request drops both and releases",
            llm_spec(DataClass::Mail),
            queued_chat(Some(task(DataClass::Mail))),
            SessionIn::Closed,
            Phase::Closed,
            vec![SessionOut::DropTurn, SessionOut::Release(model())],
        ),
    ]
}
