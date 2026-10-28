//! A Unix stream socket whose frames are `porter_wire` envelopes (a 4-byte length, then
//! JSON): the fd `Inference1.Open` returns. A session writes `ClientFrame`s and reads
//! `InferEvent`s, a memfd riding on the frame that names it as SCM_RIGHTS.

use crate::error::TransportError;
use porter_wire::{decode_frame, encode_frame};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use tokio::io::{AsyncReadExt, AsyncWriteExt, Interest};
use tokio::net::UnixStream;

use porter_infer::{ClientFrame, InferEvent, InferSession, SessionError};
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
use std::io::IoSlice;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream as StdStream;

/// The most descriptors one frame may carry (a computer-use step names one frame; a chat turn
/// a handful of images).
pub const MAX_ATTACHMENTS: usize = 16;

/// A Unix stream socket, framed with `porter_wire`'s envelope.
#[derive(Debug)]
pub struct FramedSession {
    stream: UnixStream,
    /// Bytes read and not yet a whole frame; kept across calls so `next` is cancel safe.
    inbox: Vec<u8>,
}

impl FramedSession {
    /// A session over `fd`, a socket. Needs a tokio runtime.
    pub(crate) fn over(fd: OwnedFd) -> Result<Self, TransportError> {
        let std_stream = StdStream::from(fd);
        std_stream
            .set_nonblocking(true)
            .and_then(|()| UnixStream::from_std(std_stream))
            .map(|stream| Self {
                stream,
                inbox: Vec::new(),
            })
            .map_err(|e| TransportError::Malformed(format!("not a socket: {e}")))
    }

    /// Reads one frame of any body. Cancel safe: a partial frame stays in the session.
    pub(crate) async fn read_body<T: serde::de::DeserializeOwned>(
        &mut self,
    ) -> Result<T, TransportError> {
        loop {
            match decode_frame::<T>(&self.inbox) {
                Ok(Some((envelope, used))) => {
                    self.inbox.drain(..used);
                    return Ok(envelope.body);
                }
                Ok(None) => {}
                Err(error) => return Err(TransportError::Malformed(error.to_string())),
            }
            match self.stream.read_buf(&mut self.inbox).await {
                Ok(0) | Err(_) => return Err(TransportError::Closed),
                Ok(_) => {}
            }
        }
    }
}

/// A transport failure as the session's.
fn session_error(error: TransportError) -> SessionError {
    match error {
        TransportError::Closed => SessionError::Closed,
        TransportError::Malformed(why) => SessionError::Malformed(why),
        other => SessionError::Malformed(other.to_string()),
    }
}

impl FramedSession {
    /// Writes `frame` with `attachments` (memfds) riding on it as SCM_RIGHTS. A frame names
    /// one by its index in `attachments` (`ImageSource::Attached`), and exactly as many must be
    /// given as the frame names (`ClientFrame::attachments`): the daemon takes that many from
    /// the descriptors it received, in order. Not cancel safe: a dropped call may leave half a
    /// frame on the socket, so end the session after one.
    pub async fn send_with(
        &mut self,
        frame: ClientFrame,
        attachments: &[OwnedFd],
    ) -> Result<(), SessionError> {
        let named = frame.attachments();
        if attachments.len() != named || named > MAX_ATTACHMENTS {
            return Err(SessionError::Malformed(format!(
                "the frame names {named} attachments, {} were given (the limit is {MAX_ATTACHMENTS})",
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

impl InferSession for FramedSession {
    async fn send(&mut self, frame: ClientFrame) -> Result<(), SessionError> {
        self.send_with(frame, &[]).await
    }

    async fn send_attached(
        &mut self,
        frame: ClientFrame,
        attachments: Vec<OwnedFd>,
    ) -> Result<(), SessionError> {
        self.send_with(frame, &attachments).await
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        self.read_body().await.map_err(session_error)
    }
}
