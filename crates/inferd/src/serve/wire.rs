//! One session's socket: frames in (with the descriptors that ride on them), events out.
//!
//! A frame's descriptors are attributed by order, not by where the kernel delivered them: every
//! descriptor received joins one queue, and a frame that names `n` attachments takes the first
//! `n` (`ClientFrame::attachments`). A frame that finds fewer, or a stream that ends with
//! descriptors nobody named, is a protocol error and ends the session.

use porter_core::wire::{FrameRead, decode_frame, encode_frame};
use porter_infer::{ClientFrame, InferEvent};
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};
use std::collections::VecDeque;
use std::io::IoSliceMut;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use tokio::io::{AsyncWriteExt, Interest};
use tokio::net::UnixStream;

/// The most descriptors one frame may carry and the most the queue may hold unclaimed.
pub const MAX_QUEUED_FDS: usize = 32;

/// Not porter's protocol (or the stream broke): the session ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Broken;

/// A whole frame and the descriptors it named.
pub type Framed = (ClientFrame, Vec<OwnedFd>);

/// A session's socket with the bytes and descriptors read so far.
#[derive(Debug)]
pub struct Wire {
    stream: UnixStream,
    inbox: Vec<u8>,
    fds: VecDeque<OwnedFd>,
}

impl Wire {
    /// Wraps the daemon's end of the socketpair `Open` made.
    pub fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            inbox: Vec::new(),
            fds: VecDeque::new(),
        }
    }

    /// The next frame. Cancel safe: what was read stays in the wire.
    pub async fn next_frame(&mut self) -> Result<Option<Framed>, Broken> {
        loop {
            match decode_frame::<ClientFrame>(&self.inbox) {
                Ok(FrameRead::Complete(envelope, used)) => {
                    self.inbox.drain(..used);
                    return self.claim(envelope.body);
                }
                Ok(FrameRead::Partial) => {}
                Err(_) => return Err(Broken),
            }
            match self.read_some().await {
                Ok(0) => {
                    return match (self.inbox.is_empty(), self.fds.is_empty()) {
                        (true, true) => Ok(None),
                        _ => Err(Broken),
                    };
                }
                Ok(_) => {}
                Err(_) => return Err(Broken),
            }
        }
    }

    /// Takes the descriptors `frame` names off the queue.
    fn claim(&mut self, frame: ClientFrame) -> Result<Option<Framed>, Broken> {
        let named = frame.attachments();
        if self.fds.len() < named {
            return Err(Broken);
        }
        let taken: Vec<OwnedFd> = self.fds.drain(..named).collect();
        Ok(Some((frame, taken)))
    }

    /// One read: bytes into the inbox, descriptors onto the queue. The byte count, 0 at end.
    async fn read_some(&mut self) -> std::io::Result<usize> {
        let (bytes, fds) = self
            .stream
            .async_io(Interest::READABLE, || {
                let mut chunk = [0_u8; 8192];
                let mut space =
                    [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_QUEUED_FDS))];
                let mut control = RecvAncillaryBuffer::new(&mut space);
                let got = recvmsg(
                    &self.stream,
                    &mut [IoSliceMut::new(&mut chunk)],
                    &mut control,
                    RecvFlags::CMSG_CLOEXEC,
                )
                .map_err(std::io::Error::from)?;
                let fds: Vec<OwnedFd> = control
                    .drain()
                    .flat_map(|message| match message {
                        RecvAncillaryMessage::ScmRights(fds) => fds.collect::<Vec<_>>(),
                        _ => Vec::new(),
                    })
                    .collect();
                Ok((chunk[..got.bytes].to_vec(), fds))
            })
            .await?;
        let count = bytes.len();
        self.inbox.extend(bytes);
        self.fds.extend(fds);
        match self.fds.len() > MAX_QUEUED_FDS {
            true => Err(std::io::Error::other("too many descriptors queued")),
            false => Ok(count),
        }
    }

    /// Writes one event. Not cancel safe, and not raced against reads: a client that stops
    /// reading holds only its own session.
    pub async fn write(&mut self, event: &InferEvent) -> std::io::Result<()> {
        let bytes = encode_frame(event).map_err(std::io::Error::other)?;
        self.stream.write_all(&bytes).await
    }
}
