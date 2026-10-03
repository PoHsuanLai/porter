//! The session `Inference1.Open` returns: a Unix socket whose frames are `porter_core::wire`
//! envelopes (a 4-byte length, then JSON). The client writes `ClientFrame`s and reads
//! `InferEvent`s; a memfd rides on the frame that names it as SCM_RIGHTS.

use crate::error::TransportError;
use porter_core::wire::{FrameRead, decode_frame, encode_frame};
use porter_infer::{ClientFrame, InferEvent, InferSession, SessionError};
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
use std::io::IoSlice;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream as StdStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt, Interest};
use tokio::net::UnixStream;

/// The most descriptors one frame may carry (a computer-use step names one frame; a chat turn
/// a handful of images).
pub const MAX_ATTACHMENTS: usize = 16;

/// The fd `Inference1.Open` returned, framed with `porter_core::wire`'s envelope.
#[derive(Debug)]
pub struct DbusSession {
    stream: UnixStream,
    /// Bytes read and not yet a whole frame; kept across calls so `next` is cancel safe.
    inbox: Vec<u8>,
}

impl DbusSession {
    /// A session over `fd`, the socket `Open` returned. Needs a tokio runtime.
    pub(crate) fn over(fd: OwnedFd) -> Result<Self, TransportError> {
        let std_stream = StdStream::from(fd);
        std_stream
            .set_nonblocking(true)
            .and_then(|()| UnixStream::from_std(std_stream))
            .map(|stream| Self {
                stream,
                inbox: Vec::new(),
            })
            .map_err(|e| TransportError::Malformed(format!("the Open fd is not a socket: {e}")))
    }

    /// Writes `frame` with `attachments` (memfds) riding on it as SCM_RIGHTS. A frame names
    /// one by its index in `attachments` (`ImageSource::Attached`). Not cancel safe: a dropped
    /// call may leave half a frame on the socket, so end the session after one.
    pub async fn send_with(
        &mut self,
        frame: ClientFrame,
        attachments: &[OwnedFd],
    ) -> Result<(), SessionError> {
        if attachments.len() > MAX_ATTACHMENTS {
            return Err(SessionError::Malformed(format!(
                "{} attachments on one frame; the limit is {MAX_ATTACHMENTS}",
                attachments.len()
            )));
        }
        let bytes = encode_frame(&frame).map_err(|e| SessionError::Malformed(e.to_string()))?;
        let sent = self.write_first(&bytes, attachments).await?;
        self.stream
            .write_all(&bytes[sent..])
            .await
            .map_err(|_| SessionError::Closed)
    }

    /// The first write: the fds travel with its first byte, so it must carry at least one.
    async fn write_first(&self, bytes: &[u8], fds: &[OwnedFd]) -> Result<usize, SessionError> {
        self.stream
            .async_io(Interest::WRITABLE, || {
                let mut space =
                    [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_ATTACHMENTS))];
                let mut control = SendAncillaryBuffer::new(&mut space);
                let borrowed: Vec<_> = fds.iter().map(AsFd::as_fd).collect();
                if !borrowed.is_empty() {
                    control.push(SendAncillaryMessage::ScmRights(&borrowed));
                }
                sendmsg(
                    &self.stream,
                    &[IoSlice::new(bytes)],
                    &mut control,
                    SendFlags::NOSIGNAL,
                )
                .map_err(std::io::Error::from)
            })
            .await
            .map_err(|_| SessionError::Closed)
    }
}

impl InferSession for DbusSession {
    async fn send(&mut self, frame: ClientFrame) -> Result<(), SessionError> {
        self.send_with(frame, &[]).await
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        loop {
            match decode_frame::<InferEvent>(&self.inbox) {
                Ok(FrameRead::Complete(envelope, used)) => {
                    self.inbox.drain(..used);
                    return Ok(envelope.body);
                }
                Ok(FrameRead::Partial) => {}
                Err(error) => return Err(SessionError::Malformed(error.to_string())),
            }
            match self.stream.read_buf(&mut self.inbox).await {
                Ok(0) | Err(_) => return Err(SessionError::Closed),
                Ok(_) => {}
            }
        }
    }
}
