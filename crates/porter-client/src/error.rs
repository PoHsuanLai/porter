//! porter-client's errors.

use porter_core::AccountId;
use porter_core::wire::Refusal;
#[cfg(feature = "infer")]
use porter_infer::{InferRefusal, SessionError};

/// Why a transport could not carry a request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// No daemon answered.
    #[error("no account service reachable")]
    Unreachable,
    /// The connection closed mid-request.
    #[error("connection closed")]
    Closed,
    /// The daemon refused the caller: the bus's `AccessDenied`, with the daemon's own text (its
    /// caller table does not name this program, or the app holds no grant for the call). Asking
    /// again does not help; the person or the packager must change who may call.
    #[error("refused by the daemon: {0}")]
    Denied(String),
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
    /// `add_account`: the person signed in to an account that was already here, so nothing was
    /// added. It is that account; ask for a grant of it (`find`, then `request_grant`).
    #[error("that account is already added: {0}")]
    AlreadyAdded(AccountId),
    /// inferd refused (feature `infer`).
    #[cfg(feature = "infer")]
    #[error(transparent)]
    InferRefused(#[from] InferRefusal),
    /// The reply does not answer the request (a daemon of another version).
    #[error("reply does not answer the request")]
    Mismatched,
}

#[cfg(feature = "infer")]
impl From<SessionError> for TransportError {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::Closed => TransportError::Closed,
            SessionError::Malformed(why) => TransportError::Malformed(why),
        }
    }
}
