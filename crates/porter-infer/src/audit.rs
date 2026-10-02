//! The audit record of one request: who, through what, how much; never the content.

use crate::reply::TokenUsage;
use porter_core::{AccountId, AppId, Bytes, Count, Locality, ModelId, UnixSeconds};
use serde::{Deserialize, Serialize};

/// One audited request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// When.
    pub at: UnixSeconds,
    /// The app.
    pub app: AppId,
    /// The account.
    pub account: AccountId,
    /// The model.
    pub model: ModelId,
    /// Where it ran.
    pub locality: Locality,
    /// Tokens spent.
    pub usage: TokenUsage,
    /// Bytes that left the machine (zero on device).
    pub bytes_out: Bytes,
    /// Frames sent to the model: "a frame was sent", never the frame.
    pub images: Count,
    /// Milliseconds of audio sent or produced: "audio was sent", never the audio or its text.
    pub audio_ms: Count,
}
