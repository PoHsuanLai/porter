//! The rows of the route, the wait for the engine and the first request of an idle session.

#![allow(unused_imports)]
use super::helpers::*;
use porter_core::consent::Usage;
use porter_core::{DataClass, Permille};
use porter_infer::{
    ClientFrame, Declined, DeclinedBecause, InferEvent, InferRefusal, InferReply, ModelError,
    ModelRef, PickRefusal, Readiness, RequestKind, ShowReason, Why,
};

fn old() -> ModelRef {
    ModelRef {
        account: porter_core::AccountId::parse("local").expect("id"),
        model: porter_core::ModelId::parse("old").expect("id"),
    }
}

fn declined() -> Declined {
    Declined {
        model: old(),
        because: DeclinedBecause::NoRoom,
    }
}

pub(super) fn rows() -> Vec<Row> {
    let failed = |e| done(InferReply::Failed(e));
    vec![
        (
            "route refused closes",
            llm_spec(DataClass::Mail),
            Phase::Opened,
            SessionIn::Routed(Err(InferRefusal::NeedsGrant.into())),
            Phase::Closed,
            vec![done(InferReply::Refused(InferRefusal::NeedsGrant))],
        ),
        (
            "a named model that cannot serve says which and why, then refuses",
            llm_spec(DataClass::Mail),
            Phase::Opened,
            SessionIn::Routed(Err(PickRefusal {
                refusal: InferRefusal::Unavailable,
                declined: Some(declined()),
            })),
            Phase::Closed,
            vec![
                SessionOut::Emit(InferEvent::Declined(declined())),
                done(InferReply::Refused(InferRefusal::Unavailable)),
            ],
        ),
        (
            "the reason goes out first when the person asked to see it",
            llm_spec(DataClass::Mail),
            Phase::Opened,
            decided_why(Readiness::Loadable, Why::Warm, ShowReason::On),
            waiting(None),
            vec![
                SessionOut::Emit(InferEvent::Why(Why::Warm)),
                SessionOut::Emit(InferEvent::Waiting(Readiness::Loadable)),
                SessionOut::Want(model()),
            ],
        ),
        (
            "an eviction is announced even with the reasons off, before the wait",
            llm_spec(DataClass::Mail),
            Phase::Opened,
            decided_why(
                Readiness::Loadable,
                Why::Evicted { model: old() },
                ShowReason::Off,
            ),
            waiting(None),
            vec![
                SessionOut::Emit(InferEvent::Why(Why::Evicted { model: old() })),
                SessionOut::Emit(InferEvent::Waiting(Readiness::Loadable)),
                SessionOut::Want(model()),
            ],
        ),
        (
            "route chosen waits for the engine",
            llm_spec(DataClass::Mail),
            Phase::Opened,
            decided(Readiness::Ready),
            waiting(None),
            vec![SessionOut::Want(model())],
        ),
        (
            "route chosen with the engine loading tells the client first",
            llm_spec(DataClass::Mail),
            Phase::Opened,
            decided(Readiness::Loadable),
            waiting(None),
            vec![
                SessionOut::Emit(InferEvent::Waiting(Readiness::Loadable)),
                SessionOut::Want(model()),
            ],
        ),
        (
            "download progress while waiting is shown",
            llm_spec(DataClass::Mail),
            waiting(None),
            SessionIn::EngineProgress(Readiness::Downloading(Permille(420))),
            waiting(None),
            vec![SessionOut::Emit(InferEvent::Waiting(
                Readiness::Downloading(Permille(420)),
            ))],
        ),
        (
            "ready progress while waiting says nothing",
            llm_spec(DataClass::Mail),
            waiting(None),
            SessionIn::EngineProgress(Readiness::Ready),
            waiting(None),
            vec![],
        ),
        (
            "progress outside waiting is ignored",
            llm_spec(DataClass::Mail),
            idle(RoutedNote::Sent),
            SessionIn::EngineProgress(Readiness::Loading),
            idle(RoutedNote::Sent),
            vec![],
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
            vec![routed(), SessionOut::StartTurn(task(DataClass::Mail))],
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
            "the first turn says who answers",
            llm_spec(DataClass::Mail),
            idle(RoutedNote::Pending),
            request(task(DataClass::Mail)),
            in_turn(RequestKind::Task, AudioCursor::NoAudio),
            vec![routed(), SessionOut::StartTurn(task(DataClass::Mail))],
        ),
        (
            "a refused first request does not spend the routed note",
            llm_spec(DataClass::Mail),
            idle(RoutedNote::Pending),
            request(speak(DataClass::Mail)),
            idle(RoutedNote::Pending),
            vec![unsupported()],
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
            "closed stays closed",
            llm_spec(DataClass::Mail),
            Phase::Closed,
            request(task(DataClass::Mail)),
            Phase::Closed,
            vec![],
        ),
    ]
}
