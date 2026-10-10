//! porter-core's one error: values from outside that are not values of ours.

/// Why a value from outside (a file, the bus, a socket) was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// Text that should have been an identifier of the named kind is not one.
    #[error("malformed {what}: {text:?}")]
    MalformedId {
        /// The kind of identifier (`"account id"`, `"app name"`).
        what: &'static str,
        /// The text refused.
        text: String,
    },
    /// A wire frame could not be decoded; the connection that sent it is closed.
    #[error("malformed frame: {0}")]
    MalformedFrame(String),
    /// A frame longer than the protocol allows.
    #[error("frame of {0} bytes exceeds the limit")]
    FrameTooLong(usize),
}
