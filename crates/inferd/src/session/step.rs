//! The transition function of one session (see the parent module for the states).

use super::{
    AudioCursor, CuaProgress, Phase, RoutedNote, SessionIn, SessionOut, SessionSpec, fits, model_of,
};
use crate::speech::check_audio;
use porter_infer::{
    AudioRate, ClientFrame, InferEvent, InferRefusal, InferReply, InferRequest, ModelError,
    Readiness, RequestKind, ServedBy,
};

/// The next state and effects for `input` in `phase` (models §4.2, voice §3.4).
///
/// The fd server feeds it the route's decision, the engine's readiness, the client's frames and
/// the running turn's events, and carries out what comes back; the machine never waits.
pub fn step(spec: &SessionSpec, phase: Phase, input: SessionIn) -> (Phase, Vec<SessionOut>) {
    match (phase, input) {
        (Phase::Closed, _) => (Phase::Closed, vec![]),
        (phase, SessionIn::Closed) => close(phase),
        (Phase::Opened, SessionIn::Routed(Err(refusal))) => {
            (Phase::Closed, vec![finished(InferReply::Refused(refusal))])
        }
        (Phase::Opened, SessionIn::Routed(Ok(decision))) => {
            let wait = waiting_event(decision.readiness);
            let want = SessionOut::Want(model_of(&decision.served));
            (
                Phase::Waiting {
                    served: decision.served,
                    queued: None,
                },
                wait.into_iter().chain([want]).collect(),
            )
        }
        (phase @ Phase::Waiting { .. }, SessionIn::EngineProgress(readiness)) => {
            (phase, waiting_event(readiness).into_iter().collect())
        }
        (Phase::Waiting { served, queued }, SessionIn::EngineReady) => {
            let idle = Phase::Idle {
                served,
                routed: RoutedNote::Pending,
                cua: CuaProgress::NotBegun,
            };
            match queued {
                Some(request) => request_in_idle(spec, idle, request),
                None => (idle, vec![]),
            }
        }
        (Phase::Waiting { served, .. }, SessionIn::EngineFailed) => (
            Phase::Closed,
            vec![
                finished(InferReply::Failed(ModelError::NotReady)),
                SessionOut::Release(model_of(&served)),
            ],
        ),
        (Phase::Waiting { served, queued }, SessionIn::Frame(frame)) => match (frame, queued) {
            (ClientFrame::Request(request), None) => (
                Phase::Waiting {
                    served,
                    queued: Some(request),
                },
                vec![],
            ),
            (ClientFrame::Cancel, Some(_)) => (
                Phase::Waiting {
                    served,
                    queued: None,
                },
                vec![finished(InferReply::Cancelled)],
            ),
            (ClientFrame::Cancel, None) => (
                Phase::Waiting {
                    served,
                    queued: None,
                },
                vec![],
            ),
            (_, queued) => (
                Phase::Waiting { served, queued },
                vec![refused(InferRefusal::Unsupported)],
            ),
        },
        (idle @ Phase::Idle { .. }, SessionIn::Frame(ClientFrame::Request(request))) => {
            request_in_idle(spec, idle, request)
        }
        (idle @ Phase::Idle { .. }, SessionIn::Frame(ClientFrame::Cancel)) => (idle, vec![]),
        (idle @ Phase::Idle { .. }, SessionIn::Frame(_)) => {
            (idle, vec![refused(InferRefusal::Unsupported)])
        }
        (Phase::Idle { served, .. }, SessionIn::EngineFailed) => {
            (Phase::Closed, vec![SessionOut::Release(model_of(&served))])
        }
        (opened @ Phase::Opened, SessionIn::Frame(ClientFrame::Request(_))) => {
            (opened, vec![refused(InferRefusal::Unsupported)])
        }
        (phase @ Phase::InTurn { .. }, input) => in_turn(spec, phase, input),
        (phase, _) => (phase, vec![]),
    }
}

fn finished(reply: InferReply) -> SessionOut {
    SessionOut::Emit(InferEvent::Finished(reply))
}

fn refused(refusal: InferRefusal) -> SessionOut {
    finished(InferReply::Refused(refusal))
}

/// What the client sees while an engine is not ready: nothing once it is.
fn waiting_event(readiness: Readiness) -> Option<SessionOut> {
    match readiness {
        Readiness::Ready => None,
        other => Some(SessionOut::Emit(InferEvent::Waiting(other))),
    }
}

/// Who a phase is pinned to, if it is pinned.
fn pinned(phase: &Phase) -> Option<&ServedBy> {
    match phase {
        Phase::Waiting { served, .. }
        | Phase::Idle { served, .. }
        | Phase::InTurn { served, .. } => Some(served),
        Phase::Opened | Phase::Closed => None,
    }
}

/// End of file or an explicit close: drop the running turn, give the engine back.
fn close(phase: Phase) -> (Phase, Vec<SessionOut>) {
    let drop_turn = matches!(phase, Phase::InTurn { .. }).then_some(SessionOut::DropTurn);
    let release = pinned(&phase).map(model_of).map(SessionOut::Release);
    (
        Phase::Closed,
        drop_turn.into_iter().chain(release).collect(),
    )
}

/// What a session of this class may carry: everything on it has the class it was opened with,
/// a computer-use session carries only `Screen`, and a step needs a begun run.
fn admits(spec: &SessionSpec, cua: CuaProgress, request: &InferRequest) -> bool {
    let carried = match request {
        InferRequest::Chat(r) => Some(r.class),
        InferRequest::Embed(r) => Some(r.class),
        InferRequest::Task(r) => Some(r.class),
        InferRequest::Speak(r) => Some(r.class),
        InferRequest::CuaBegin(_) | InferRequest::CuaStep(_) | InferRequest::Transcribe(_) => None,
    };
    let kind = request.kind();
    let cua_session = matches!(kind, RequestKind::CuaBegin | RequestKind::CuaStep);
    let cua_ok = !cua_session || crate::cua_run::check_class(spec.class).is_ok();
    let begun = kind != RequestKind::CuaStep || cua == CuaProgress::Begun;
    cua_ok && begun && carried.is_none_or(|class| class == spec.class) && fits(&spec.need, kind)
}

/// A request arrives in an idle (or just-readied) session. A turn that starts says who answers
/// first, once.
fn request_in_idle(
    spec: &SessionSpec,
    idle: Phase,
    request: InferRequest,
) -> (Phase, Vec<SessionOut>) {
    let Phase::Idle {
        served,
        routed,
        cua,
    } = &idle
    else {
        return (idle, vec![]);
    };
    if !admits(spec, *cua, &request) {
        return (idle, vec![refused(InferRefusal::Unsupported)]);
    }
    let kind = request.kind();
    let audio = match kind {
        RequestKind::Transcribe => AudioCursor::Expecting { next: 0 },
        _ => AudioCursor::NoAudio,
    };
    let routed_event = match routed {
        RoutedNote::Pending => Some(SessionOut::Emit(InferEvent::Routed(served.clone()))),
        RoutedNote::Sent => None,
    };
    let next = Phase::InTurn {
        served: served.clone(),
        kind,
        audio,
        cua: *cua,
        queued: None,
    };
    (
        next,
        routed_event
            .into_iter()
            .chain([SessionOut::StartTurn(request)])
            .collect(),
    )
}

/// The only audio rate of v1 (`AudioRate` docs).
const V1_RATE: AudioRate = AudioRate(16_000);

/// A turn is over: back to idle, then the queued request, if any, starts at once.
fn turn_over(
    spec: &SessionSpec,
    ended: Ended,
    effects: Vec<SessionOut>,
) -> (Phase, Vec<SessionOut>) {
    let Ended {
        served,
        cua,
        queued,
    } = ended;
    let idle = Phase::Idle {
        served,
        routed: RoutedNote::Sent,
        cua,
    };
    match queued {
        None => (idle, effects),
        Some(request) => {
            let (next, started) = request_in_idle(spec, idle, request);
            (next, effects.into_iter().chain(started).collect())
        }
    }
}

/// What a finished turn leaves behind.
struct Ended {
    served: ServedBy,
    cua: CuaProgress,
    queued: Option<InferRequest>,
}

fn in_turn(spec: &SessionSpec, phase: Phase, input: SessionIn) -> (Phase, Vec<SessionOut>) {
    let Phase::InTurn {
        served,
        kind,
        audio,
        cua,
        queued,
    } = phase
    else {
        return (phase, vec![]);
    };
    let ended = |cua| Ended {
        served: served.clone(),
        cua,
        queued: queued.clone(),
    };
    let same = |audio, queued| Phase::InTurn {
        served: served.clone(),
        kind,
        audio,
        cua,
        queued,
    };
    match input {
        SessionIn::TurnEvent(event) => (same(audio, queued), vec![SessionOut::Emit(event)]),
        SessionIn::TurnDone(reply) => {
            let begun = kind == RequestKind::CuaBegin && matches!(reply, InferReply::CuaStep(_));
            let cua = if begun { CuaProgress::Begun } else { cua };
            turn_over(
                spec,
                ended(cua),
                vec![SessionOut::Audit(reply.clone()), finished(reply)],
            )
        }
        SessionIn::Frame(ClientFrame::Cancel) => turn_over(
            spec,
            ended(cua),
            vec![
                SessionOut::DropTurn,
                SessionOut::Audit(InferReply::Cancelled),
                finished(InferReply::Cancelled),
            ],
        ),
        SessionIn::Frame(ClientFrame::Request(request)) => {
            match (&queued, admits(spec, cua, &request)) {
                (None, true) => (same(audio, Some(request)), vec![]),
                _ => (
                    same(audio, queued),
                    vec![refused(InferRefusal::Unsupported)],
                ),
            }
        }
        SessionIn::Frame(ClientFrame::Audio(_) | ClientFrame::EndOfAudio)
            if kind != RequestKind::Transcribe =>
        {
            (
                same(audio, queued),
                vec![refused(InferRefusal::Unsupported)],
            )
        }
        SessionIn::Frame(ClientFrame::Audio(frame)) => match check_audio(audio, V1_RATE, &frame) {
            Ok(next) => (same(next, queued), vec![SessionOut::Audio(frame)]),
            Err(error) => turn_over(
                spec,
                ended(cua),
                vec![
                    SessionOut::DropTurn,
                    SessionOut::Audit(InferReply::Failed(error)),
                    finished(InferReply::Failed(error)),
                ],
            ),
        },
        SessionIn::Frame(ClientFrame::EndOfAudio) => (
            same(AudioCursor::NoAudio, queued),
            vec![SessionOut::EndAudio],
        ),
        SessionIn::EngineFailed => {
            // The queued request never ran; it is told, as the running one is.
            let for_queued = queued
                .is_some()
                .then(|| finished(InferReply::Failed(ModelError::NotReady)));
            (
                Phase::Closed,
                [
                    SessionOut::DropTurn,
                    finished(InferReply::Failed(ModelError::NotReady)),
                ]
                .into_iter()
                .chain(for_queued)
                .chain([SessionOut::Release(model_of(&served))])
                .collect(),
            )
        }
        SessionIn::Closed => close(same(audio, queued)),
        SessionIn::Routed(_) | SessionIn::EngineReady | SessionIn::EngineProgress(_) => {
            (same(audio, queued), vec![])
        }
    }
}
