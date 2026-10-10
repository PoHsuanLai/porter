//! The client side of one `Open` fd: frames out, events in. `porter-client` re-exports it and
//! each transport supplies its own session; `porter-fake` supplies a scripted one.

use crate::event::{ClientFrame, InferEvent};
use std::future::Future;

/// Why a session could not carry a frame.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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

    /// Writes one frame with descriptors (memfds) riding on it as SCM_RIGHTS: a frame names one
    /// by its index in `attachments` (`ImageSource::Attached`). A session that carries none
    /// refuses a non-empty list. Unix only: a descriptor is not a thing elsewhere.
    #[cfg(unix)]
    fn send_attached(
        &mut self,
        frame: ClientFrame,
        attachments: Vec<std::os::fd::OwnedFd>,
    ) -> impl Future<Output = Result<(), SessionError>> + Send {
        async move {
            match attachments.is_empty() {
                true => self.send(frame).await,
                false => Err(SessionError::Malformed(
                    "this session carries no attachments".to_owned(),
                )),
            }
        }
    }

    /// The next event; `Err(Closed)` after the daemon ends the session.
    fn next(&mut self) -> impl Future<Output = Result<InferEvent, SessionError>> + Send;
}
