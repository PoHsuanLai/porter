//! A session over a Unix stream socket whose frames are `porter_core::wire` envelopes (a 4-byte
//! length, then JSON): the fd `Inference1.Open` returns, and a connection to the latchkey
//! socket after its hello. The client writes `ClientFrame`s and reads `InferEvent`s; a memfd
//! rides on the frame that names it as SCM_RIGHTS.

#[cfg(feature = "dbus")]
use crate::error::TransportError;
use porter_core::wire::{FrameRead, decode_frame, encode_frame};
use porter_infer::{ClientFrame, InferEvent, InferSession, SessionError};
use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags, recvmsg, sendmsg,
};
use std::io::IoSlice;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
#[cfg(feature = "dbus")]
use std::os::unix::net::UnixStream as StdStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt, Interest};
use tokio::net::UnixStream;

/// The most descriptors one frame may carry (a computer-use step names one frame; a chat turn
/// a handful of images).
pub const MAX_ATTACHMENTS: usize = 16;

/// A Unix stream socket, framed with `porter_core::wire`'s envelope.
#[derive(Debug)]
pub struct FramedSession {
    stream: UnixStream,
    /// Bytes read and not yet a whole frame; kept across calls so `next` is cancel safe.
    inbox: Vec<u8>,
}

impl FramedSession {
    /// A session over `fd`, a socket. Needs a tokio runtime.
    #[cfg(feature = "dbus")]
    pub(crate) fn over(fd: OwnedFd) -> Result<Self, TransportError> {
        let std_stream = StdStream::from(fd);
        std_stream
            .set_nonblocking(true)
            .and_then(|()| UnixStream::from_std(std_stream))
            .map(Self::from_stream)
            .map_err(|e| TransportError::Malformed(format!("not a socket: {e}")))
    }

    /// A session over a connected stream.
    pub(crate) fn from_stream(stream: UnixStream) -> Self {
        Self {
            stream,
            inbox: Vec::new(),
        }
    }

    /// Writes one frame of any body (the hello, or an accountd call).
    pub(crate) async fn write_body<T: serde::Serialize>(
        &mut self,
        body: &T,
    ) -> Result<(), SessionError> {
        let bytes = encode_frame(body).map_err(|e| SessionError::Malformed(e.to_string()))?;
        self.stream
            .write_all(&bytes)
            .await
            .map_err(|_| SessionError::Closed)
    }

    /// Reads one frame of any body. Cancel safe: a partial frame stays in the session.
    pub(crate) async fn read_body<T: serde::de::DeserializeOwned>(
        &mut self,
    ) -> Result<T, SessionError> {
        loop {
            match decode_frame::<T>(&self.inbox) {
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

    /// Reads one frame of any body with the descriptors that rode on the bytes of it (a relay's
    /// end, on `AccountsReply::Authenticated`). Every descriptor received is returned, in
    /// order, so the caller can refuse a reply that brings too many or too few.
    pub(crate) async fn read_body_with_fds<T: serde::de::DeserializeOwned>(
        &mut self,
    ) -> Result<(T, Vec<OwnedFd>), SessionError> {
        let mut fds = Vec::new();
        loop {
            match decode_frame::<T>(&self.inbox) {
                Ok(FrameRead::Complete(envelope, used)) => {
                    self.inbox.drain(..used);
                    return Ok((envelope.body, fds));
                }
                Ok(FrameRead::Partial) => {}
                Err(error) => return Err(SessionError::Malformed(error.to_string())),
            }
            let mut chunk = [0u8; 4096];
            let (read, mut received) = self
                .stream
                .async_io(Interest::READABLE, || {
                    let mut space =
                        [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_ATTACHMENTS))];
                    let mut control = RecvAncillaryBuffer::new(&mut space);
                    let message = recvmsg(
                        &self.stream,
                        &mut [std::io::IoSliceMut::new(&mut chunk)],
                        &mut control,
                        RecvFlags::CMSG_CLOEXEC,
                    )
                    .map_err(std::io::Error::from)?;
                    let received: Vec<OwnedFd> = control
                        .drain()
                        .filter_map(|m| match m {
                            RecvAncillaryMessage::ScmRights(rights) => Some(rights),
                            _ => None,
                        })
                        .flatten()
                        .collect();
                    Ok((message.bytes, received))
                })
                .await
                .map_err(|_| SessionError::Closed)?;
            fds.append(&mut received);
            if read == 0 {
                return Err(SessionError::Closed);
            }
            self.inbox.extend_from_slice(&chunk[..read]);
        }
    }

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
        self.read_body().await
    }
}
