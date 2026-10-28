//! Why a frame could not be written or read.

/// A frame that could not be encoded or decoded; the connection that sent it is closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FrameError {
    /// The body could not be written as JSON, or the bytes are not an envelope of the body's type.
    #[error("malformed frame: {0}")]
    Malformed(String),
    /// A frame longer than the protocol allows.
    #[error("frame of {0} bytes exceeds the limit")]
    TooLong(usize),
}
