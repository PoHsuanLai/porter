//! A model turn in flight: a task that pushes [`TurnStep`]s down a channel. Dropping the turn
//! aborts the task, which drops the HTTP future and closes the engine's stream.

use porter_infer::{AudioFrame, InferReply, ModelError};
use porter_router::seams::{RunningTurn, TurnStep};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// A turn in flight.
#[derive(Debug)]
pub struct Turn {
    steps: mpsc::UnboundedReceiver<TurnStep>,
    task: JoinHandle<()>,
    /// Where a `Transcribe` turn's audio goes; none once the person stopped talking.
    audio: Option<mpsc::UnboundedSender<AudioFrame>>,
}

impl Turn {
    /// A turn that is the task `task`, whose steps arrive on `steps`; it takes no audio.
    pub fn running(steps: mpsc::UnboundedReceiver<TurnStep>, task: JoinHandle<()>) -> Self {
        Self {
            steps,
            task,
            audio: None,
        }
    }

    /// A turn that is the task `task`, whose steps arrive on `steps` and which reads the audio
    /// frames sent to `audio` (a `Transcribe` turn); the turn's `end_audio` closes that channel.
    pub fn listening(
        steps: mpsc::UnboundedReceiver<TurnStep>,
        task: JoinHandle<()>,
        audio: mpsc::UnboundedSender<AudioFrame>,
    ) -> Self {
        Self {
            steps,
            task,
            audio: Some(audio),
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RunningTurn for Turn {
    fn audio(&mut self, frame: AudioFrame) {
        if let Some(audio) = &self.audio {
            let _ = audio.send(frame);
        }
    }

    fn end_audio(&mut self) {
        self.audio = None;
    }

    async fn next(&mut self) -> TurnStep {
        match self.steps.recv().await {
            Some(step) => step,
            // The task ended without an answer: it was aborted or it panicked.
            None => TurnStep::Done(InferReply::Failed(ModelError::Unreachable)),
        }
    }
}
