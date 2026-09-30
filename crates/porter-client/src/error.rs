//! porter-client's errors.

use porter_core::wire::Refusal;
use porter_infer::InferRefusal;

/// Why a transport could not carry a request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// No daemon answered.
    #[error("no account service reachable")]
    Unreachable,
    /// The connection closed mid-request.
    #[error("connection closed")]
    Closed,
    /// The other side sent something that is not porter's protocol.
    #[error("malformed reply: {0}")]
    Malformed(String),
}

/// Why a client call failed; each variant is something an app can show or act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    /// The request did not arrive or the reply did not come back.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// accountd refused.
    #[error("refused: {0:?}")]
    Refused(Refusal),
    /// inferd refused.
    #[error(transparent)]
    InferRefused(#[from] InferRefusal),
    /// The reply does not answer the request (a daemon of another version).
    #[error("reply does not answer the request")]
    Mismatched,
}
