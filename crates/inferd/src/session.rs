//! One `Open` fd as a pure machine: the states of a session and what each event does to it
//! (models §4.2, with the audio rules of voice §3.4). `step` takes the state and one input and
//! returns the next state and the effects the fd server carries out; it never waits.
//!
//! The types and the `fits` table are built; `step` is a `todo!()` listed in `FINDINGS.md`.

use porter_core::capability::SpeechMode;
use porter_core::{DataClass, Need, Tier};
use porter_infer::{ClientFrame, InferEvent, InferReply, InferRequest, ModelRef, RequestKind};

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

/// The next state and effects for `input` in `phase`.
pub fn step(spec: &SessionSpec, phase: Phase, input: SessionIn) -> (Phase, Vec<SessionOut>) {
    let _ = (spec, phase, input);
    todo!(
        "the rows of models §4.2 and voice §3.4: route, wait, idle, turn, cancel, audio rules, close"
    )
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
