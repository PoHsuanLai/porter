//! Running a plan stage by stage. Each stage is announced before it runs (`Why` when the person
//! asked to see reasons, `Routed` for who it is sent to, and a `Stage` note for the footer), then
//! run by the seam that serves its role: a [`Transcriber`] for `Hear`, a turn runner for `Answer`.
//! The transcript joins the request as text, so the answer stage sees what the person said as
//! words and nothing else about the audio.

use crate::serve::{RunningTurn, TurnRunner, TurnStep};
use crate::speech::{AudioIn, AudioPull};
use porter_infer::{
    AudioFrame, ChatMessage, ChatRequest, ChatSink, Flow, InferEvent, InferReply, InferRequest,
    MessagePart, ModelError, Pipeline, Refusal, Role, ShowReason, Stage, StageNote, StageRole,
    TranscribeBegin, TranscribeReply, Why,
};
use std::collections::VecDeque;
use std::future::Future;

/// What a speech-to-text stage runs on (stoker's `SpeechToText` behind the engine the stage's
/// model names; a scripted one in tests). It is [`crate::speech::SpeechRunner::transcribe`]'s shape.
pub trait Transcriber: Send + Sync {
    /// Reads `audio` until it ends, sends `Heard` events to `sink`, and gives the transcript.
    fn transcribe<A: AudioIn, S: ChatSink>(
        &self,
        stage: &Stage,
        begin: &TranscribeBegin,
        audio: &mut A,
        sink: &mut S,
    ) -> impl Future<Output = Result<TranscribeReply, ModelError>> + Send;
}

/// Audio that is already all here.
#[derive(Debug, Clone, Default)]
pub struct VecAudio(pub VecDeque<AudioFrame>);

impl AudioIn for VecAudio {
    async fn next(&mut self) -> AudioPull {
        match self.0.pop_front() {
            Some(frame) => AudioPull::Frame(frame),
            None => AudioPull::End,
        }
    }
}

/// What the request carries into the pipeline.
#[derive(Debug, Clone)]
pub struct PipelineInput {
    /// The audio, when the request has any.
    pub audio: Option<(TranscribeBegin, VecAudio)>,
    /// The chat the answering model is given; a transcript is appended to its last user message.
    pub chat: ChatRequest,
}

/// Why a plan cannot run here yet, as a typed refusal (none for a plan this build runs).
pub fn unsupported(pipeline: &Pipeline) -> Option<Refusal> {
    pipeline.stages.iter().find_map(|stage| match stage.role {
        StageRole::Describe => Some(Refusal::NotYet("describing images with the image_in slot")),
        StageRole::Speak => Some(Refusal::NotYet(
            "speaking the answer with the voice_out slot",
        )),
        StageRole::Hear | StageRole::Answer => None,
    })
}

fn announce(stage: &Stage, show: ShowReason, sink: &mut impl ChatSink) -> Flow {
    let evicting = matches!(stage.picked.why, Why::Evicted { .. });
    let why =
        (show == ShowReason::On || evicting).then(|| InferEvent::Why(stage.picked.why.clone()));
    let note = InferEvent::Stage(StageNote {
        role: stage.role,
        served: stage.served(),
        why: stage.picked.why.clone(),
    });
    let events = why
        .into_iter()
        .chain([InferEvent::Routed(stage.served()), note]);
    for event in events {
        if sink.event(event) == Flow::Stop {
            return Flow::Stop;
        }
    }
    Flow::Continue
}

/// The chat with `transcript` joined to its last user message (or as a new one).
fn with_transcript(mut chat: ChatRequest, transcript: String) -> ChatRequest {
    let part = MessagePart::Text(transcript);
    match chat
        .messages
        .iter_mut()
        .rev()
        .find(|message| message.role == Role::User)
    {
        Some(message) => message.parts.push(part),
        None => chat.messages.push(ChatMessage {
            role: Role::User,
            parts: vec![part],
        }),
    }
    chat
}

/// Runs `pipeline`: `Hear` through `hear`, then `Answer` through `answer`. A stage this build
/// does not run is refused up front (`unsupported`), before any stage is announced or run.
pub async fn run_pipeline<H: Transcriber, T: TurnRunner>(
    pipeline: &Pipeline,
    show: ShowReason,
    input: PipelineInput,
    hear: &H,
    answer: &T,
    sink: &mut impl ChatSink,
) -> InferReply {
    if unsupported(pipeline).is_some() {
        return InferReply::Refused(porter_infer::InferRefusal::Unsupported);
    }
    let PipelineInput {
        mut audio,
        mut chat,
    } = input;
    for stage in &pipeline.stages {
        if announce(stage, show, sink) == Flow::Stop {
            return InferReply::Cancelled;
        }
        match stage.role {
            StageRole::Hear => {
                let Some((begin, frames)) = audio.as_mut() else {
                    return InferReply::Refused(porter_infer::InferRefusal::Unsupported);
                };
                match hear.transcribe(stage, begin, frames, sink).await {
                    Ok(heard) => chat = with_transcript(chat, heard.text),
                    Err(error) => return InferReply::Failed(error),
                }
            }
            StageRole::Answer => return answer_turn(answer, chat, sink).await,
            StageRole::Describe | StageRole::Speak => {
                return InferReply::Refused(porter_infer::InferRefusal::Unsupported);
            }
        }
    }
    InferReply::Refused(porter_infer::InferRefusal::Unavailable)
}

async fn answer_turn<T: TurnRunner>(
    answer: &T,
    chat: ChatRequest,
    sink: &mut impl ChatSink,
) -> InferReply {
    let mut turn = answer.start(InferRequest::Chat(chat), Vec::new());
    loop {
        match turn.next().await {
            TurnStep::Event(event) => {
                if sink.event(event) == Flow::Stop {
                    return InferReply::Cancelled;
                }
            }
            TurnStep::Done(reply) => return reply,
        }
    }
}
