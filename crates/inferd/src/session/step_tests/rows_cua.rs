//! The rows of a computer-use run: a step needs a begin.

use super::helpers::*;
use porter_core::DataClass;
use porter_infer::{ClientFrame, InferReply, RequestKind};

pub(super) fn rows() -> Vec<Row> {
    vec![
        (
            "a computer-use step before a begin is refused",
            cua_spec(DataClass::Screen),
            idle(RoutedNote::Sent),
            request(cua_step()),
            idle(RoutedNote::Sent),
            vec![unsupported()],
        ),
        (
            "a begin that is acknowledged begins the run",
            cua_spec(DataClass::Screen),
            in_turn(RequestKind::CuaBegin, AudioCursor::NoAudio),
            SessionIn::TurnDone(begun_ack()),
            idle_cua(RoutedNote::Sent, CuaProgress::Begun),
            vec![SessionOut::Audit(begun_ack()), done(begun_ack())],
        ),
        (
            "a begin that fails leaves the run unbegun",
            cua_spec(DataClass::Screen),
            in_turn(RequestKind::CuaBegin, AudioCursor::NoAudio),
            SessionIn::TurnDone(lost_reply()),
            idle(RoutedNote::Sent),
            vec![SessionOut::Audit(lost_reply()), done(lost_reply())],
        ),
        (
            "a begin that is cancelled leaves the run unbegun",
            cua_spec(DataClass::Screen),
            in_turn(RequestKind::CuaBegin, AudioCursor::NoAudio),
            frame(ClientFrame::Cancel),
            idle(RoutedNote::Sent),
            vec![
                SessionOut::DropTurn,
                SessionOut::Audit(InferReply::Cancelled),
                done(InferReply::Cancelled),
            ],
        ),
        (
            "a step after a begin starts",
            cua_spec(DataClass::Screen),
            idle_cua(RoutedNote::Sent, CuaProgress::Begun),
            request(cua_step()),
            in_turn_with(
                RequestKind::CuaStep,
                AudioCursor::NoAudio,
                CuaProgress::Begun,
                None,
            ),
            vec![SessionOut::StartTurn(cua_step())],
        ),
        (
            "a step keeps the run begun, even when it fails",
            cua_spec(DataClass::Screen),
            in_turn_with(
                RequestKind::CuaStep,
                AudioCursor::NoAudio,
                CuaProgress::Begun,
                None,
            ),
            SessionIn::TurnDone(lost_reply()),
            idle_cua(RoutedNote::Sent, CuaProgress::Begun),
            vec![SessionOut::Audit(lost_reply()), done(lost_reply())],
        ),
        (
            "a second begin starts another run",
            cua_spec(DataClass::Screen),
            idle_cua(RoutedNote::Sent, CuaProgress::Begun),
            request(cua_begin()),
            in_turn_with(
                RequestKind::CuaBegin,
                AudioCursor::NoAudio,
                CuaProgress::Begun,
                None,
            ),
            vec![SessionOut::StartTurn(cua_begin())],
        ),
        (
            "a step queued behind a begin that fails is refused when it comes up",
            cua_spec(DataClass::Screen),
            in_turn_with(
                RequestKind::CuaBegin,
                AudioCursor::NoAudio,
                CuaProgress::NotBegun,
                Some(cua_step()),
            ),
            SessionIn::TurnDone(lost_reply()),
            idle(RoutedNote::Sent),
            vec![
                SessionOut::Audit(lost_reply()),
                done(lost_reply()),
                unsupported(),
            ],
        ),
    ]
}
