//! The transition function of one session (see the parent module for the states).

use super::{
    AudioCursor, CuaProgress, EngineNow, HeardAudio, MAX_HEARD_MS, Phase, RoutedNote, Routing,
    SessionIn, SessionOut, SessionSpec, fits, model_of,
};
use crate::audio::{audio_ms, check_audio};
use porter_core::Need;
use porter_infer::{
    AudioRate, ClientFrame, Declined, InferEvent, InferRefusal, InferReply, InferRequest,
    ModelError, Readiness, RequestKind, ServedBy, ShowReason, StageNote, TranscribeBegin, Why,
};

/// The next state and effects for `input` in `phase` (models §4.2, voice §3.4).
///
/// The fd server feeds it the route's decision, the engine's readiness, the client's frames and
/// the running turn's events, and carries out what comes back; the machine never waits.
pub fn step(spec: &SessionSpec, phase: Phase, input: SessionIn) -> (Phase, Vec<SessionOut>) {
    match (phase, input) {
        (Phase::Closed, _) => (Phase::Closed, vec![]),
        (phase, SessionIn::Closed) => close(phase),
        (phase @ Phase::Hearing { .. }, input) => hearing(spec, phase, input),
        (
            waiting @ Phase::Waiting { queued: None, .. },
            SessionIn::Frame(ClientFrame::Request(InferRequest::Transcribe(begin))),
        ) if matches!(spec.need, Need::Llm(_)) => start_hearing(waiting, &begin),
        (Phase::Opened, SessionIn::Routed(Err(refused))) => (
            Phase::Closed,
            declined_event(refused.declined)
                .into_iter()
                .chain([finished(InferReply::Refused(refused.refusal))])
                .collect(),
        ),
        (Phase::Opened, SessionIn::Routed(Ok(decision))) => {
            let why = why_event(&decision);
            let wait = waiting_event(decision.readiness);
            let want = SessionOut::Want(model_of(&decision.served));
            let answer = decision.answer_note();
            (
                Phase::Waiting {
                    served: decision.served,
                    answer,
                    queued: None,
                },
                why.into_iter().chain(wait).chain([want]).collect(),
            )
        }
        (phase @ Phase::Waiting { .. }, SessionIn::EngineProgress(readiness)) => {
            (phase, waiting_event(readiness).into_iter().collect())
        }
        (
            Phase::Waiting {
                served,
                answer,
                queued,
            },
            SessionIn::EngineReady,
        ) => {
            let idle = Phase::Idle {
                served,
                answer,
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
        (
            Phase::Waiting {
                served,
                answer,
                queued,
            },
            SessionIn::Frame(frame),
        ) => match (frame, queued) {
            (ClientFrame::Request(request), None) => (
                Phase::Waiting {
                    served,
                    answer,
                    queued: Some(request),
                },
                vec![],
            ),
            (ClientFrame::Cancel, Some(_)) => (
                Phase::Waiting {
                    served,
                    answer,
                    queued: None,
                },
                vec![finished(InferReply::Cancelled)],
            ),
            (ClientFrame::Cancel, None) => (
                Phase::Waiting {
                    served,
                    answer,
                    queued: None,
                },
                vec![],
            ),
            (_, queued) => (
                Phase::Waiting {
                    served,
                    answer,
                    queued,
                },
                vec![refused(InferRefusal::Unsupported)],
            ),
        },
        (
            idle @ Phase::Idle { .. },
            SessionIn::Frame(ClientFrame::Request(InferRequest::Transcribe(begin))),
        ) if matches!(spec.need, Need::Llm(_)) => start_hearing(idle, &begin),
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

/// The reason, before anything else: announced when the person asked to see it, and always when
/// a model is unloaded for this one (a swap is never hidden).
fn why_event(decision: &Routing) -> Vec<SessionOut> {
    let evicting = matches!(decision.why, Why::Evicted { .. });
    let announce = decision.show == ShowReason::On || evicting;
    let reached = decision
        .reached
        .iter()
        .filter(|_| decision.show == ShowReason::On);
    announce
        .then_some(&decision.why)
        .into_iter()
        .chain(reached)
        .map(|why| SessionOut::Emit(InferEvent::Why(why.clone())))
        .collect()
}

/// Which named model could not serve, ahead of the refusal that ends the session.
fn declined_event(declined: Option<Declined>) -> Option<SessionOut> {
    declined.map(|declined| SessionOut::Emit(InferEvent::Declined(declined)))
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
        | Phase::Hearing { served, .. }
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
    let cua_ok = !cua_session || crate::router::check_class(spec.class).is_ok();
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
        answer,
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
    // Every answer says who gave it before its first token, even a plain one-stage answer.
    let answer_note = matches!(kind, RequestKind::Chat | RequestKind::Task)
        .then(|| SessionOut::Emit(InferEvent::Stage(answer.clone())));
    let next = Phase::InTurn {
        served: served.clone(),
        answer: answer.clone(),
        kind,
        audio,
        cua: *cua,
        queued: None,
    };
    (
        next,
        routed_event
            .into_iter()
            .chain(answer_note)
            .chain([SessionOut::StartTurn(request)])
            .collect(),
    )
}

/// The only audio rate of v1 (`AudioRate` docs).
const V1_RATE: AudioRate = AudioRate(16_000);

/// A voice chat begins on a language session, idle or still waiting for its engine: its
/// `Transcribe` request says how the audio that follows is to be heard, and the audio is kept
/// (up to [`MAX_HEARD_MS`]) until the chat it belongs to. Nothing is announced yet. A rate other
/// than v1's is refused, as a transcription's frames would be.
fn start_hearing(before: Phase, begin: &TranscribeBegin) -> (Phase, Vec<SessionOut>) {
    let (served, answer, routed, cua, engine) = match before {
        Phase::Idle {
            served,
            answer,
            routed,
            cua,
        } => (served, answer, routed, cua, EngineNow::Ready),
        Phase::Waiting {
            served,
            answer,
            queued: None,
        } => (
            served,
            answer,
            RoutedNote::Pending,
            CuaProgress::NotBegun,
            EngineNow::Loading,
        ),
        other => return (other, vec![refused(InferRefusal::Unsupported)]),
    };
    let hearing = Phase::Hearing {
        served,
        answer,
        routed,
        cua,
        heard: HeardAudio {
            begin: begin.clone(),
            frames: Vec::new(),
        },
        audio: AudioCursor::Expecting { next: 0 },
        engine,
        chat: None,
    };
    if begin.rate == V1_RATE {
        (hearing, vec![])
    } else {
        (settle(hearing), vec![refused(InferRefusal::Unsupported)])
    }
}

/// The phase a `Hearing` session goes back to when no voice chat is under way.
fn settle(hearing: Phase) -> Phase {
    match hearing {
        Phase::Hearing {
            served,
            answer,
            routed,
            cua,
            engine: EngineNow::Ready,
            ..
        } => Phase::Idle {
            served,
            answer,
            routed,
            cua,
        },
        Phase::Hearing { served, answer, .. } => Phase::Waiting {
            served,
            answer,
            queued: None,
        },
        other => other,
    }
}

/// What a language session does while it collects the audio of a voice chat: frames fill the
/// buffer, `EndOfAudio` closes it, the `Chat` that follows starts the pipeline turn (once the
/// engine is up). Anything else is refused and changes nothing, except a cancel, which drops the
/// audio, and a failure of the engine, which ends the session.
fn hearing(spec: &SessionSpec, phase: Phase, input: SessionIn) -> (Phase, Vec<SessionOut>) {
    let Phase::Hearing {
        served,
        answer,
        routed,
        cua,
        heard,
        audio,
        engine,
        chat,
    } = phase
    else {
        return (phase, vec![]);
    };
    let ended = audio == AudioCursor::NoAudio;
    let same = |heard, audio, engine, chat| Phase::Hearing {
        served: served.clone(),
        answer: answer.clone(),
        routed,
        cua,
        heard,
        audio,
        engine,
        chat,
    };
    let back = |heard: HeardAudio| settle(same(heard, audio, engine, chat.clone()));
    let begin_turn = |request: InferRequest, heard: HeardAudio| {
        (
            Phase::InTurn {
                served: served.clone(),
                answer: answer.clone(),
                kind: RequestKind::Chat,
                audio: AudioCursor::NoAudio,
                cua,
                queued: None,
            },
            vec![SessionOut::Hear(heard), SessionOut::StartTurn(request)],
        )
    };
    match input {
        SessionIn::Frame(ClientFrame::Cancel) => {
            (back(heard), vec![finished(InferReply::Cancelled)])
        }
        SessionIn::Frame(ClientFrame::Audio(frame)) if !ended => {
            match check_audio(audio, V1_RATE, &frame) {
                Ok(next @ AudioCursor::Expecting { next: samples })
                    if audio_ms(V1_RATE, samples) <= MAX_HEARD_MS =>
                {
                    let mut heard = heard;
                    heard.frames.push(frame);
                    (same(heard, next, engine, chat), vec![])
                }
                Ok(_) | Err(_) => (
                    back(heard),
                    vec![finished(InferReply::Failed(ModelError::Unreadable))],
                ),
            }
        }
        SessionIn::Frame(ClientFrame::EndOfAudio) if !ended && heard.frames.is_empty() => (
            back(heard),
            vec![finished(InferReply::Failed(ModelError::Unreadable))],
        ),
        SessionIn::Frame(ClientFrame::EndOfAudio) if !ended => {
            (same(heard, AudioCursor::NoAudio, engine, chat), vec![])
        }
        SessionIn::Frame(ClientFrame::Request(request @ InferRequest::Chat(_)))
            if ended && chat.is_none() && admits(spec, cua, &request) =>
        {
            match engine {
                EngineNow::Ready => begin_turn(request, heard),
                EngineNow::Loading => (same(heard, audio, engine, Some(request)), vec![]),
            }
        }
        SessionIn::EngineReady => match chat {
            Some(request) => begin_turn(request, heard),
            None => (same(heard, audio, EngineNow::Ready, None), vec![]),
        },
        SessionIn::EngineProgress(readiness) if engine == EngineNow::Loading => (
            same(heard, audio, engine, chat),
            waiting_event(readiness).into_iter().collect(),
        ),
        SessionIn::EngineFailed => (
            Phase::Closed,
            [
                finished(InferReply::Failed(ModelError::NotReady)),
                SessionOut::Release(model_of(&served)),
            ]
            .into_iter()
            .collect(),
        ),
        SessionIn::Frame(_) => (
            same(heard, audio, engine, chat),
            vec![refused(InferRefusal::Unsupported)],
        ),
        SessionIn::Routed(_)
        | SessionIn::EngineProgress(_)
        | SessionIn::TurnEvent(_)
        | SessionIn::TurnDone(_)
        | SessionIn::Closed => (same(heard, audio, engine, chat), vec![]),
    }
}

/// A turn is over: back to idle, then the queued request, if any, starts at once.
fn turn_over(
    spec: &SessionSpec,
    ended: Ended,
    effects: Vec<SessionOut>,
) -> (Phase, Vec<SessionOut>) {
    let Ended {
        served,
        answer,
        cua,
        queued,
    } = ended;
    let idle = Phase::Idle {
        served,
        answer,
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
    answer: StageNote,
    cua: CuaProgress,
    queued: Option<InferRequest>,
}

fn in_turn(spec: &SessionSpec, phase: Phase, input: SessionIn) -> (Phase, Vec<SessionOut>) {
    let Phase::InTurn {
        served,
        answer,
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
        answer: answer.clone(),
        cua,
        queued: queued.clone(),
    };
    let same = |audio, queued| Phase::InTurn {
        served: served.clone(),
        answer: answer.clone(),
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
