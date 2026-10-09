//! Speech turns: the runners that connect a turn to a speech engine. The engines are stoker's
//! `speech-provider` backends, closed enums in inferd: [`SttBackend`] is built (one arm,
//! [`speech_host_client::SpeechHostClient`] over the engine's Unix socket; `stt.rs`), `TtsBackend`
//! is not (`speak` below). The audio rules every `Transcribe` turn obeys (`check_audio`,
//! `audio_ms`, `AudioIn`) moved to `porter_router::audio` and stay importable from here.
//!
//! inferd keeps no audio after a turn: frames live in the buffer of the running engine call
//! and are dropped with it, on `Cancel` too.

use crate::supervise::Supervised;
use engine_supervisor::EngineId;
use porter_infer::{ChatSink, InferRefusal, ModelError, ServedBy, SpeakReply, SpeakRequest};

mod stt;

/// The audio rules, moved to `porter_router::audio`.
pub use porter_router::audio::{AudioIn, AudioPull, MAX_FRAME_MS, audio_ms, check_audio};
pub use stt::{ChannelAudio, Ears, SttBackend, SttShape};

/// Runs speech turns against the engine the route chose: the turn's model, who is told it
/// answered, and the supervisor that is told the engine is in use. Built per turn
/// ([`SpeechRunner::for_model`]).
#[derive(Debug, Clone)]
pub struct SpeechRunner {
    backend: SttBackend,
    request: SttShape,
    served: ServedBy,
    engines: Supervised,
    engine: EngineId,
}

impl SpeechRunner {
    /// A text-to-speech turn: `Spoken` events into `sink`; a sink answering `Stop` is
    /// barge-in.
    pub async fn speak(
        &self,
        request: &SpeakRequest,
        sink: &mut impl ChatSink,
    ) -> Result<SpeakReply, ModelError> {
        // Not built (`TtsBackend` over speech_provider::TextToSpeech is the lane that fills
        // it): the engine is not ready, and no audio is made.
        let _ = (request, sink, InferRefusal::Unsupported);
        Err(ModelError::NotReady)
    }
}
