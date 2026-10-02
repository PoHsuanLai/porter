//! A scripted inference session: plays canned events for each request kind, records every frame
//! the client sent, and releases a transcript's events as the audio passes their sample index
//! (the shape of stoker's `ScriptedStt`).

use porter_infer::{
    ClientFrame, InferEvent, InferRefusal, InferReply, InferSession, RequestKind, SessionError,
};
use std::collections::VecDeque;

/// One step of a scripted turn.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptStep {
    /// Emit this event as soon as the turn starts (or the previous step is released).
    Emit(InferEvent),
    /// Emit this event once the audio sent so far reaches `samples` (or at `EndOfAudio`).
    AfterAudio {
        /// The number of samples that must have been sent.
        samples: u64,
        /// The event.
        event: InferEvent,
    },
}

/// The events one request kind produces.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    /// The request kind it answers.
    pub kind: RequestKind,
    /// The steps, in order. A `Finished` event ends the turn.
    pub steps: Vec<ScriptStep>,
}

/// A scripted session. Each `Request` frame consumes the first script of its kind; a request
/// with no script is answered `Finished(Refused(Unsupported))`.
#[derive(Debug, Default)]
pub struct FakeInferSession {
    scripts: VecDeque<Script>,
    running: VecDeque<ScriptStep>,
    ready: VecDeque<InferEvent>,
    audio_samples: u64,
    sent: Vec<ClientFrame>,
}

impl FakeInferSession {
    /// A session that plays these scripts.
    pub fn scripted(scripts: impl IntoIterator<Item = Script>) -> Self {
        Self {
            scripts: scripts.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Every frame the client sent, in order.
    pub fn sent(&self) -> &[ClientFrame] {
        &self.sent
    }

    /// Moves steps whose condition holds into the ready queue.
    fn release(&mut self, ended: bool) {
        while let Some(step) = self.running.front() {
            let due = match step {
                ScriptStep::Emit(_) => true,
                ScriptStep::AfterAudio { samples, .. } => ended || self.audio_samples >= *samples,
            };
            if !due {
                break;
            }
            if let Some(ScriptStep::Emit(event) | ScriptStep::AfterAudio { event, .. }) =
                self.running.pop_front()
            {
                self.ready.push_back(event);
            }
        }
    }
}

impl InferSession for FakeInferSession {
    async fn send(&mut self, frame: ClientFrame) -> Result<(), SessionError> {
        self.sent.push(frame.clone());
        match frame {
            ClientFrame::Request(request) => {
                let at = self.scripts.iter().position(|s| s.kind == request.kind());
                match at.and_then(|i| self.scripts.remove(i)) {
                    Some(script) => {
                        self.audio_samples = 0;
                        self.running = script.steps.into();
                        self.release(false);
                    }
                    None => self
                        .ready
                        .push_back(InferEvent::Finished(InferReply::Refused(
                            InferRefusal::Unsupported,
                        ))),
                }
            }
            ClientFrame::Cancel => {
                self.running.clear();
                self.ready
                    .push_back(InferEvent::Finished(InferReply::Cancelled));
            }
            ClientFrame::Audio(frame) => {
                // S16LE mono: two bytes per sample.
                self.audio_samples += (frame.pcm.0.len() / 2) as u64;
                self.release(false);
            }
            ClientFrame::EndOfAudio => self.release(true),
        }
        Ok(())
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        self.ready.pop_front().ok_or(SessionError::Closed)
    }
}
