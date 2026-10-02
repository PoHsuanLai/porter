//! One `Open` fd as a pure machine: the states of a session and what each event does to it
//! (models §4.2, with the audio rules of voice §3.4). `step` takes the state and one input and
//! returns the next state and the effects the fd server carries out; it never waits.
//!
//! The types, the `fits` table and `step` are built; the table test is in `session_step_tests`.

use crate::speech::check_audio;
use porter_core::capability::SpeechMode;
use porter_core::{DataClass, Need, Tier};
use porter_infer::{
    AudioRate, ClientFrame, InferEvent, InferRefusal, InferReply, InferRequest, ModelError,
    ModelRef, RequestKind,
};

/// What the session was opened for: fixed for its life, so the route is chosen once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    /// The need the session was opened with.
    pub need: Need,
    /// The data class of everything on it.
    pub class: DataClass,
    /// The tier the app asked for.
    pub tier: Tier,
}

/// Whether the `Routed` event has gone out yet (it goes out once, when the engine is ready).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutedNote {
    /// Not yet.
    Pending,
    /// Sent.
    Sent,
}

/// Where the audio of a `Transcribe` turn has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCursor {
    /// This turn takes no audio.
    NoAudio,
    /// Audio is expected from this sample on; a frame at any other index is out of order.
    Expecting {
        /// The next sample index.
        next: u64,
    },
}

/// The state of one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Open; the route has not been decided.
    Opened,
    /// Routed; the engine is not ready. At most one request waits.
    Waiting {
        /// The model the route chose.
        chosen: ModelRef,
        /// A request that arrived meanwhile (queue depth one).
        queued: Option<InferRequest>,
    },
    /// Ready for a request.
    Idle {
        /// The model the session is pinned to.
        chosen: ModelRef,
        /// Whether `Routed` was sent.
        routed: RoutedNote,
    },
    /// A turn is running.
    InTurn {
        /// The model the session is pinned to.
        chosen: ModelRef,
        /// What kind of request is running.
        kind: RequestKind,
        /// Audio progress, for `Transcribe`.
        audio: AudioCursor,
    },
    /// Over: the fd closed or the route refused.
    Closed,
}

/// What the fd server feeds the machine.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionIn {
    /// The route decided: a model, or why none.
    Routed(Result<ModelRef, porter_infer::InferRefusal>),
    /// The chosen engine is ready.
    EngineReady,
    /// The chosen engine failed to start.
    EngineFailed,
    /// A frame from the client.
    Frame(ClientFrame),
    /// An event the running turn produced (deltas, tool calls, usage, heard text).
    TurnEvent(InferEvent),
    /// The running turn ended with this reply.
    TurnDone(InferReply),
    /// End of file or an explicit close.
    Closed,
}

/// What the fd server must do.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionOut {
    /// Ask the supervisor for this model's engine.
    Want(ModelRef),
    /// Tell the supervisor the session no longer needs it.
    Release(ModelRef),
    /// Write this event to the client.
    Emit(InferEvent),
    /// Start the model future for this request.
    StartTurn(InferRequest),
    /// Drop the running model future (closes the engine's stream).
    DropTurn,
    /// Meter spend and write the audit entry for the finished turn.
    Audit(InferReply),
}

/// The next state and effects for `input` in `phase` (models §4.2, voice §3.4).
///
/// `Routed` and `Waiting` events are the fd server's to send: it holds the `ServedBy` and the
/// readiness that this machine does not.
pub fn step(spec: &SessionSpec, phase: Phase, input: SessionIn) -> (Phase, Vec<SessionOut>) {
    match (phase, input) {
        (Phase::Closed, _) => (Phase::Closed, vec![]),
        (phase, SessionIn::Closed) => close(phase),
        (Phase::Opened, SessionIn::Routed(Err(refusal))) => {
            (Phase::Closed, vec![finished(InferReply::Refused(refusal))])
        }
        (Phase::Opened, SessionIn::Routed(Ok(chosen))) => (
            Phase::Waiting {
                chosen: chosen.clone(),
                queued: None,
            },
            vec![SessionOut::Want(chosen)],
        ),
        (Phase::Waiting { chosen, queued }, SessionIn::EngineReady) => {
            let idle = Phase::Idle {
                chosen,
                routed: RoutedNote::Pending,
            };
            match queued {
                Some(request) => request_in_idle(spec, idle, request),
                None => (idle, vec![]),
            }
        }
        (Phase::Waiting { chosen, .. }, SessionIn::EngineFailed) => (
            Phase::Closed,
            vec![
                finished(InferReply::Failed(ModelError::NotReady)),
                SessionOut::Release(chosen),
            ],
        ),
        (Phase::Waiting { chosen, queued }, SessionIn::Frame(frame)) => match (frame, queued) {
            (ClientFrame::Request(request), None) => (
                Phase::Waiting {
                    chosen,
                    queued: Some(request),
                },
                vec![],
            ),
            (ClientFrame::Cancel, Some(_)) => (
                Phase::Waiting {
                    chosen,
                    queued: None,
                },
                vec![finished(InferReply::Cancelled)],
            ),
            (ClientFrame::Cancel, None) => (
                Phase::Waiting {
                    chosen,
                    queued: None,
                },
                vec![],
            ),
            (_, queued) => (
                Phase::Waiting { chosen, queued },
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
        (Phase::Idle { chosen, .. }, SessionIn::EngineFailed) => {
            (Phase::Closed, vec![SessionOut::Release(chosen)])
        }
        (opened @ Phase::Opened, SessionIn::Frame(ClientFrame::Request(_))) => {
            (opened, vec![refused(InferRefusal::Unsupported)])
        }
        (phase @ Phase::InTurn { .. }, input) => in_turn(phase, input),
        (phase, _) => (phase, vec![]),
    }
}

fn finished(reply: InferReply) -> SessionOut {
    SessionOut::Emit(InferEvent::Finished(reply))
}

fn refused(refusal: InferRefusal) -> SessionOut {
    finished(InferReply::Refused(refusal))
}

/// The model a phase is pinned to, if it has one.
fn pinned(phase: &Phase) -> Option<&ModelRef> {
    match phase {
        Phase::Waiting { chosen, .. }
        | Phase::Idle { chosen, .. }
        | Phase::InTurn { chosen, .. } => Some(chosen),
        Phase::Opened | Phase::Closed => None,
    }
}

/// End of file or an explicit close: drop the running turn, give the engine back.
fn close(phase: Phase) -> (Phase, Vec<SessionOut>) {
    let drop_turn = matches!(phase, Phase::InTurn { .. }).then_some(SessionOut::DropTurn);
    let release = pinned(&phase).cloned().map(SessionOut::Release);
    (
        Phase::Closed,
        drop_turn.into_iter().chain(release).collect(),
    )
}

/// What a session of this class may carry: everything on it has the class it was opened with,
/// and a computer-use session carries only `Screen`.
fn admits(spec: &SessionSpec, request: &InferRequest) -> bool {
    let carried = match request {
        InferRequest::Chat(r) => Some(r.class),
        InferRequest::Embed(r) => Some(r.class),
        InferRequest::Task(r) => Some(r.class),
        InferRequest::Speak(r) => Some(r.class),
        InferRequest::CuaBegin(_) | InferRequest::CuaStep(_) | InferRequest::Transcribe(_) => None,
    };
    let cua = matches!(request.kind(), RequestKind::CuaBegin | RequestKind::CuaStep);
    let cua_ok = !cua || crate::cua_run::check_class(spec.class).is_ok();
    cua_ok && carried.is_none_or(|class| class == spec.class) && fits(&spec.need, request.kind())
}

/// A request arrives in an idle (or just-readied) session.
fn request_in_idle(
    spec: &SessionSpec,
    idle: Phase,
    request: InferRequest,
) -> (Phase, Vec<SessionOut>) {
    let Phase::Idle { chosen, .. } = &idle else {
        return (idle, vec![]);
    };
    if !admits(spec, &request) {
        return (idle, vec![refused(InferRefusal::Unsupported)]);
    }
    let kind = request.kind();
    let audio = match kind {
        RequestKind::Transcribe => AudioCursor::Expecting { next: 0 },
        _ => AudioCursor::NoAudio,
    };
    let next = Phase::InTurn {
        chosen: chosen.clone(),
        kind,
        audio,
    };
    (next, vec![SessionOut::StartTurn(request)])
}

/// The only audio rate of v1 (`AudioRate` docs).
const V1_RATE: AudioRate = AudioRate(16_000);

fn in_turn(phase: Phase, input: SessionIn) -> (Phase, Vec<SessionOut>) {
    let Phase::InTurn {
        chosen,
        kind,
        audio,
    } = phase
    else {
        return (phase, vec![]);
    };
    let idle = |chosen| Phase::Idle {
        chosen,
        routed: RoutedNote::Sent,
    };
    let same = |audio| Phase::InTurn {
        chosen: chosen.clone(),
        kind,
        audio,
    };
    match input {
        SessionIn::TurnEvent(event) => (same(audio), vec![SessionOut::Emit(event)]),
        SessionIn::TurnDone(reply) => (
            idle(chosen),
            vec![SessionOut::Audit(reply.clone()), finished(reply)],
        ),
        SessionIn::Frame(ClientFrame::Cancel) => (
            idle(chosen),
            vec![
                SessionOut::DropTurn,
                SessionOut::Audit(InferReply::Cancelled),
                finished(InferReply::Cancelled),
            ],
        ),
        SessionIn::Frame(ClientFrame::Request(_)) => {
            (same(audio), vec![refused(InferRefusal::Unsupported)])
        }
        SessionIn::Frame(ClientFrame::Audio(_) | ClientFrame::EndOfAudio)
            if kind != RequestKind::Transcribe =>
        {
            (same(audio), vec![refused(InferRefusal::Unsupported)])
        }
        SessionIn::Frame(ClientFrame::Audio(frame)) => match check_audio(audio, V1_RATE, &frame) {
            Ok(next) => (same(next), vec![]),
            Err(error) => (
                idle(chosen),
                vec![
                    SessionOut::DropTurn,
                    SessionOut::Audit(InferReply::Failed(error)),
                    finished(InferReply::Failed(error)),
                ],
            ),
        },
        SessionIn::Frame(ClientFrame::EndOfAudio) => (same(AudioCursor::NoAudio), vec![]),
        SessionIn::EngineFailed => (
            Phase::Closed,
            vec![
                SessionOut::DropTurn,
                finished(InferReply::Failed(ModelError::NotReady)),
                SessionOut::Release(chosen),
            ],
        ),
        SessionIn::Closed => close(same(audio)),
        SessionIn::Routed(_) | SessionIn::EngineReady => (same(audio), vec![]),
    }
}

/// Whether a request of this kind fits the need the session was opened with: chat and tasks
/// need a language model, embeddings an embedding model, computer-use steps a computer-use
/// model, transcription a speech model with `Stt`, speaking one with `Tts`.
pub fn fits(need: &Need, kind: RequestKind) -> bool {
    match (need, kind) {
        (Need::Llm(_), RequestKind::Chat | RequestKind::Task) => true,
        (Need::Embeddings(_), RequestKind::Embed) => true,
        (Need::ComputerUse(_), RequestKind::CuaBegin | RequestKind::CuaStep) => true,
        (Need::Speech(speech), RequestKind::Transcribe) => speech.modes.contains(&SpeechMode::Stt),
        (Need::Speech(speech), RequestKind::Speak) => speech.modes.contains(&SpeechMode::Tts),
        _ => false,
    }
}

#[cfg(test)]
#[path = "session_step_tests.rs"]
mod step_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::Tokens;
    use porter_core::capability::CuaEnv;
    use porter_core::need::{CuaNeed, DimsNeed, EmbedNeed, LlmNeed, SpeechNeed};

    fn llm() -> Need {
        Need::Llm(LlmNeed {
            features: Default::default(),
            context: Tokens(1),
        })
    }

    fn speech(modes: &[SpeechMode]) -> Need {
        Need::Speech(SpeechNeed {
            modes: modes.iter().copied().collect(),
        })
    }

    #[test]
    fn requests_fit_the_need_the_session_was_opened_with() {
        use RequestKind::*;
        let kinds = [Chat, Embed, Task, CuaBegin, CuaStep, Transcribe, Speak];
        let cases: Vec<(&str, Need, Vec<RequestKind>)> = vec![
            ("llm", llm(), vec![Chat, Task]),
            (
                "embeddings",
                Need::Embeddings(EmbedNeed {
                    dims: DimsNeed::Any,
                    modalities: Default::default(),
                }),
                vec![Embed],
            ),
            (
                "computer use",
                Need::ComputerUse(CuaNeed {
                    environments: [CuaEnv::Desktop].into(),
                }),
                vec![CuaBegin, CuaStep],
            ),
            ("speech in", speech(&[SpeechMode::Stt]), vec![Transcribe]),
            ("speech out", speech(&[SpeechMode::Tts]), vec![Speak]),
            (
                "speech both ways",
                speech(&[SpeechMode::Stt, SpeechMode::Tts]),
                vec![Transcribe, Speak],
            ),
            ("realtime alone", speech(&[SpeechMode::Realtime]), vec![]),
        ];
        for (name, need, fitting) in cases {
            for kind in kinds {
                assert_eq!(
                    fits(&need, kind),
                    fitting.contains(&kind),
                    "{name} / {kind:?}"
                );
            }
        }
    }
}
