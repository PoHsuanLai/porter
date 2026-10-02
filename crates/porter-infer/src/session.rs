//! The client side of one `Open` fd: frames out, events in. `porter-client` re-exports it and
//! each transport supplies its own session; `porter-fake` supplies a scripted one.

use crate::event::{ClientFrame, InferEvent};
use std::future::Future;

/// Why a session could not carry a frame.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// The other side closed the session.
    #[error("session closed")]
    Closed,
    /// The other side sent something that is not porter's protocol.
    #[error("malformed frame: {0}")]
    Malformed(String),
}

/// One open inference session, pinned to the model the route chose when it was opened.
pub trait InferSession: Send {
    /// Writes one frame (a request, `Cancel`, an audio frame, `EndOfAudio`).
    fn send(&mut self, frame: ClientFrame)
    -> impl Future<Output = Result<(), SessionError>> + Send;

    /// The next event; `Err(Closed)` after the daemon ends the session.
    fn next(&mut self) -> impl Future<Output = Result<InferEvent, SessionError>> + Send;
}
