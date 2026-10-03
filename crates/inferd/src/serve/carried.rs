//! What a turn's request carried, which its reply does not say: how many frames went to the
//! model and how much audio. The audit entry records both ("a frame was sent", "audio was sent":
//! never the frame, the audio or its text), so the loop keeps a [`Tally`] of the turn in flight
//! and hands the sink a [`Carried`] when the turn ends, however it ends.

use crate::speech::audio_ms;
use porter_core::Count;
use porter_infer::{AudioFrame, AudioRate, InferReply, InferRequest, MessagePart, ToolResultPart};

/// What one finished turn carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carried {
    /// Frames and images in the request: a chat's image parts (tool results' too), or the one
    /// frame of a computer-use step.
    pub images: Count,
    /// Milliseconds of audio sent to the model (a transcription) or produced by it (speech).
    pub audio_ms: Count,
}

/// The only audio rate of v1 (`AudioRate` docs), until a `Transcribe` turn names its own.
const V1_RATE: AudioRate = AudioRate(16_000);

/// The running count of the turn in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Tally {
    images: u32,
    rate: AudioRate,
    samples: u64,
}

impl Default for Tally {
    fn default() -> Self {
        Self::nothing()
    }
}

fn images_in(parts: &[MessagePart]) -> u32 {
    parts
        .iter()
        .map(|part| match part {
            MessagePart::Image(_) => 1,
            MessagePart::ToolResult(ToolResultPart { parts, .. }) => images_in(parts),
            MessagePart::Text(_) | MessagePart::ToolCall(_) | MessagePart::Thought(_) => 0,
        })
        .sum()
}

impl Tally {
    fn nothing() -> Self {
        Self {
            images: 0,
            rate: V1_RATE,
            samples: 0,
        }
    }

    /// The tally of a turn that starts with `request`.
    pub(super) fn begin(request: &InferRequest) -> Self {
        let (images, rate) = match request {
            InferRequest::Chat(chat) => (
                chat.messages.iter().map(|m| images_in(&m.parts)).sum(),
                V1_RATE,
            ),
            InferRequest::CuaStep(_) => (1, V1_RATE),
            InferRequest::Transcribe(begin) => (0, begin.rate),
            InferRequest::Embed(_)
            | InferRequest::Task(_)
            | InferRequest::CuaBegin(_)
            | InferRequest::Speak(_) => (0, V1_RATE),
        };
        Self {
            images,
            rate,
            samples: 0,
        }
    }

    /// An accepted audio frame went to the engine.
    pub(super) fn heard(&mut self, frame: &AudioFrame) {
        let samples = (frame.pcm.0.len() / 2) as u64;
        self.samples = self.samples.saturating_add(samples);
    }

    /// What the turn carried, given how it ended: the audio the reply says it produced adds to
    /// the audio that was sent.
    pub(super) fn closing(&self, reply: &InferReply) -> Carried {
        let produced = match reply {
            InferReply::Spoke(spoke) => spoke.audio_ms,
            _ => 0,
        };
        Carried {
            images: Count(self.images),
            audio_ms: Count(audio_ms(self.rate, self.samples).saturating_add(produced)),
        }
    }
}

#[cfg(test)]
mod tests;
