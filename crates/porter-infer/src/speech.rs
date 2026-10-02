//! Speech on the inference wire: audio in as frames on the `Open` fd, text or audio out as
//! events. Mono, signed 16-bit little-endian, at the stated rate (16 kHz for speech to text in
//! v1). inferd keeps no audio after the turn.

use crate::ids::Base64Bytes;
use porter_core::DataClass;
use porter_core::capability::LanguageTag;
use porter_core::consent::Usage;
use serde::{Deserialize, Serialize};
use std::fmt;

/// How the transcript arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscribeMode {
    /// Partial and final segments while the person speaks.
    Streaming,
    /// One transcript when the audio ends.
    Batch,
}

/// Which language to listen for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum LangPick {
    /// Detect it.
    Auto,
    /// These, the first being primary.
    Prefer(Vec<LanguageTag>),
}

/// Samples per second. `16_000` only in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AudioRate(pub u32);

/// Opens a speech-to-text turn. The session class must be `Voice` or the caller's own class
/// (an app transcribing its own files).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscribeBegin {
    /// Streaming or batch.
    pub mode: TranscribeMode,
    /// The language to listen for.
    pub lang: LangPick,
    /// The rate of the audio that follows.
    pub rate: AudioRate,
    /// Interactive or background.
    pub usage: Usage,
}

/// A voice an engine offers (`af_heart`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VoiceName(pub String);

/// Text to speak.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakRequest {
    /// What to say (at most 4 KiB).
    pub text: String,
    /// The voice, or the engine's default.
    pub voice: Option<VoiceName>,
    /// The language of the text.
    pub lang: LanguageTag,
    /// The class of the text: a Mail summary is never spoken by a cloud voice unless Mail's
    /// floor allows it.
    pub class: DataClass,
    /// Interactive or background.
    pub usage: Usage,
}

// The text is the person's data: Debug shows its length only.
impl fmt::Debug for SpeakRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpeakRequest")
            .field("text", &format_args!("<{} bytes>", self.text.len()))
            .field("voice", &self.voice)
            .field("lang", &self.lang)
            .field("class", &self.class)
            .field("usage", &self.usage)
            .finish()
    }
}

/// A piece of the person's voice, client to inferd. At most one second of audio per frame and
/// in order, or the turn fails with `Unreadable`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFrame {
    /// Sample index of its first sample, from the start of the turn.
    pub at: u64,
    /// S16LE mono samples at the turn's rate. Debug shows the length only.
    pub pcm: Base64Bytes,
}

/// A piece of synthesised speech, inferd to client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFrameOut {
    /// The engine's output rate.
    pub rate: AudioRate,
    /// Sample index of its first sample.
    pub at: u64,
    /// S16LE mono samples. Debug shows the length only.
    pub pcm: Base64Bytes,
}

/// What the recogniser has heard so far.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum HeardDelta {
    /// Replaces the unstable tail since sample `from`.
    Partial {
        /// The text.
        text: String,
        /// Where the replaced tail starts.
        from: u64,
    },
    /// A stable segment, never revised.
    Final {
        /// The text.
        text: String,
        /// First sample.
        from: u64,
        /// One past the last sample.
        to: u64,
    },
    /// The detected language, once.
    Lang(LanguageTag),
}

// The text is what the person said: Debug shows its length only.
impl fmt::Debug for HeardDelta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeardDelta::Partial { text, from } => {
                write!(f, "Partial(<{} bytes>, from {from})", text.len())
            }
            HeardDelta::Final { text, from, to } => {
                write!(f, "Final(<{} bytes>, {from}..{to})", text.len())
            }
            HeardDelta::Lang(tag) => write!(f, "Lang({tag:?})"),
        }
    }
}
