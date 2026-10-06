//! One `Open` fd as a pure machine: the states of a session and what each event does to it
//! (models §4.2, with the audio rules of voice §3.4). `step` takes the state and one input and
//! returns the next state and the effects the fd server carries out; it never waits.
//!
//! The types, the `fits` table and `step` are built; the tables are in `session/step_tests`.
//!
//! The machine holds who answers (`ServedBy`) so it can say so itself: `Routed` goes out once,
//! with the first turn, and `Waiting` while the engine loads. It also holds the computer-use
//! run's progress (a step before a begin is refused) and the one request that may queue behind
//! a running turn.

use porter_core::capability::SpeechMode;
use porter_core::consent::Usage;
use porter_core::{DataClass, Need, Tier};
use porter_infer::{
    AudioFrame, ClientFrame, InferEvent, InferReply, InferRequest, ModelLabel, ModelRef,
    PickRefusal, Readiness, RequestKind, ServedBy, ShowReason, StageNote, StageRole, Why,
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
    /// Whether a person is waiting (`Interactive`) or nobody is (`Background`): what accountd is
    /// asked a verdict for. It is the `usage` option of the session's `Open` (`Interactive`
    /// when the call names none).
    pub usage: Usage,
}

/// Whether the `Routed` event has gone out yet (it goes out once, with the session's first
/// turn: "sent to <provider>" says where the person's data is going, so it waits for data).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutedNote {
    /// Not yet.
    Pending,
    /// Sent.
    Sent,
}

/// How far a computer-use run has got on this session: a step needs a begun run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuaProgress {
    /// No `CuaBegin` has been accepted (every non-computer-use session stays here).
    NotBegun,
    /// A `CuaBegin` finished with an acknowledgement: steps may follow, and a new `CuaBegin`
    /// starts another run.
    Begun,
}

/// What the route decided: who answers and how ready it is. The fd server builds it from
/// `porter_infer::route` and the supervisor's readiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecision {
    /// The account, model and locality the session is pinned to.
    pub served: ServedBy,
    /// How soon it can answer; not `Ready` shows the client a `Waiting` event.
    pub readiness: Readiness,
}

/// A [`RouteDecision`] with the reason for it: what the session machine and the audit entry read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routing {
    /// The account, model and locality the session is pinned to.
    pub served: ServedBy,
    /// How soon it can answer; not `Ready` shows the client a `Waiting` event.
    pub readiness: Readiness,
    /// Why this model: announced to the client, and recorded in the audit entry.
    pub why: Why,
    /// For a hosted model, how it is reached (`Why::Reached`): announced beside `why`, so the
    /// footer can say "via OpenRouter". None for a model on this computer.
    pub reached: Option<Why>,
    /// Whether the reason is announced (`ai.auto.show_reason`); an eviction always is.
    pub show: ShowReason,
    /// The model's name as a person reads it: the catalogue entry's label. None when there is none.
    pub name: Option<ModelLabel>,
}

impl Routing {
    /// The `Answer` stage note of a turn this routing serves.
    pub fn answer_note(&self) -> StageNote {
        StageNote {
            role: StageRole::Answer,
            served: self.served.clone(),
            why: self.why.clone(),
            name: self.name.clone(),
        }
    }

    /// The decision without its reason.
    pub fn decision(&self) -> RouteDecision {
        RouteDecision {
            served: self.served.clone(),
            readiness: self.readiness,
        }
    }
}

impl From<RouteDecision> for Routing {
    /// A router that gives no reason: the model is as good as named, and nothing is announced.
    fn from(decision: RouteDecision) -> Self {
        Self {
            served: decision.served,
            readiness: decision.readiness,
            why: Why::Named,
            reached: None,
            show: ShowReason::Off,
            name: None,
        }
    }
}

/// The engine's key for a served model.
pub(crate) fn model_of(served: &ServedBy) -> ModelRef {
    ModelRef {
        account: served.account.clone(),
        model: served.model.clone(),
    }
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
        /// Who the route chose.
        served: ServedBy,
        /// The `Answer` note each chat or task turn sends before its first token.
        answer: StageNote,
        /// A request that arrived meanwhile (queue depth one).
        queued: Option<InferRequest>,
    },
    /// Ready for a request.
    Idle {
        /// Who the session is pinned to.
        served: ServedBy,
        /// The `Answer` note each chat or task turn sends before its first token.
        answer: StageNote,
        /// Whether `Routed` was sent.
        routed: RoutedNote,
        /// The computer-use run's progress.
        cua: CuaProgress,
    },
    /// A turn is running. `Routed` has gone out: a turn starts only after it.
    InTurn {
        /// Who the session is pinned to.
        served: ServedBy,
        /// The `Answer` note of this session's turns.
        answer: StageNote,
        /// What kind of request is running.
        kind: RequestKind,
        /// Audio progress, for `Transcribe`.
        audio: AudioCursor,
        /// The computer-use run's progress before this turn; a `CuaBegin` turn that ends in an
        /// acknowledgement moves it to `Begun`.
        cua: CuaProgress,
        /// A request that arrived during the turn (queue depth one); it starts when the turn
        /// ends, however it ends.
        queued: Option<InferRequest>,
    },
    /// Over: the fd closed or the route refused.
    Closed,
}

/// What the fd server feeds the machine.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionIn {
    /// The route decided: who answers, or why none.
    Routed(Result<Routing, PickRefusal>),
    /// The chosen engine is ready.
    EngineReady,
    /// The engine's readiness changed while the session waits (a download's progress); the
    /// client sees it as a `Waiting` event.
    EngineProgress(Readiness),
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
    /// Hand an accepted audio frame to the running turn's engine.
    Audio(AudioFrame),
    /// The client sent `EndOfAudio`: the running turn's engine may finish.
    EndAudio,
    /// Drop the running model future (closes the engine's stream).
    DropTurn,
    /// Meter spend and write the audit entry for the finished turn.
    Audit(InferReply),
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

mod step;
pub use step::step;

#[cfg(test)]
#[path = "session/step_tests/mod.rs"]
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
