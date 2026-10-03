//! The session `Inference1.Open` returns: a Unix socket whose frames are `porter_core::wire`
//! envelopes (a 4-byte length, then JSON). The client writes `ClientFrame`s and reads
//! `InferEvent`s; a memfd rides on the frame that names it as SCM_RIGHTS.

use super::framed::FramedSession;
use crate::error::TransportError;
use porter_infer::{ClientFrame, InferEvent, InferSession, SessionError};
use std::os::fd::OwnedFd;

pub use super::framed::MAX_ATTACHMENTS;

/// The fd `Inference1.Open` returned, framed with `porter_core::wire`'s envelope.
#[derive(Debug)]
pub struct DbusSession(FramedSession);

impl DbusSession {
    /// A session over `fd`, the socket `Open` returned. Needs a tokio runtime.
    pub(crate) fn over(fd: OwnedFd) -> Result<Self, TransportError> {
        FramedSession::over(fd).map(Self)
    }

    /// Writes `frame` with `attachments` (memfds) riding on it as SCM_RIGHTS. See
    /// [`FramedSession::send_with`]: exactly as many as the frame names, and not cancel safe.
    pub async fn send_with(
        &mut self,
        frame: ClientFrame,
        attachments: &[OwnedFd],
    ) -> Result<(), SessionError> {
        self.0.send_with(frame, attachments).await
    }
}

impl InferSession for DbusSession {
    async fn send(&mut self, frame: ClientFrame) -> Result<(), SessionError> {
        self.0.send(frame).await
    }

    async fn send_attached(
        &mut self,
        frame: ClientFrame,
        attachments: Vec<OwnedFd>,
    ) -> Result<(), SessionError> {
        self.0.send_attached(frame, attachments).await
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        self.0.next().await
    }
}
