//! A Windows named pipe whose frames are `porter_core::wire` envelopes (a 4-byte length, then
//! JSON): the connection to the latchkey agent, and with feature `infer` the session on it. The
//! twin of `framed` for a platform with no `SCM_RIGHTS`: nothing rides beside the bytes, so a
//! relay's descriptor and a frame's attached images do not exist here (the carrier refuses them;
//! see `socket/windows.rs`).
//!
//! Built only for Windows; not built or run on the machines this was written on (it is checked
//! for `x86_64-pc-windows-msvc` from Linux).

use crate::error::TransportError;
use porter_core::wire::{FrameRead, decode_frame, encode_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::NamedPipeClient;

#[cfg(feature = "infer")]
use porter_infer::{ClientFrame, InferEvent, InferSession, SessionError};

/// A pipe client, framed with `porter_core::wire`'s envelope.
#[derive(Debug)]
pub struct PipeSession {
    pipe: NamedPipeClient,
    /// Bytes read and not yet a whole frame; kept across calls so `next` is cancel safe.
    inbox: Vec<u8>,
}

impl PipeSession {
    /// A session over a connected pipe.
    pub(crate) fn from_pipe(pipe: NamedPipeClient) -> Self {
        Self {
            pipe,
            inbox: Vec::new(),
        }
    }

    /// Writes one frame of any body (the hello, or an accountd call).
    pub(crate) async fn write_body<T: serde::Serialize>(
        &mut self,
        body: &T,
    ) -> Result<(), TransportError> {
        let bytes = encode_frame(body).map_err(|e| TransportError::Malformed(e.to_string()))?;
        self.pipe
            .write_all(&bytes)
            .await
            .map_err(|_| TransportError::Closed)
    }

    /// Reads one frame of any body. Cancel safe: a partial frame stays in the session.
    pub(crate) async fn read_body<T: serde::de::DeserializeOwned>(
        &mut self,
    ) -> Result<T, TransportError> {
        loop {
            match decode_frame::<T>(&self.inbox) {
                Ok(FrameRead::Complete(envelope, used)) => {
                    self.inbox.drain(..used);
                    return Ok(envelope.body);
                }
                Ok(FrameRead::Partial) => {}
                Err(error) => return Err(TransportError::Malformed(error.to_string())),
            }
            match self.pipe.read_buf(&mut self.inbox).await {
                Ok(0) | Err(_) => return Err(TransportError::Closed),
                Ok(_) => {}
            }
        }
    }
}

#[cfg(feature = "infer")]
impl InferSession for PipeSession {
    /// A frame that names attached images is refused: there is no way to attach one here (send
    /// the images inline).
    async fn send(&mut self, frame: ClientFrame) -> Result<(), SessionError> {
        match frame.attachments() {
            0 => self.write_body(&frame).await.map_err(|error| match error {
                TransportError::Malformed(why) => SessionError::Malformed(why),
                _ => SessionError::Closed,
            }),
            named => Err(SessionError::Malformed(format!(
                "the frame names {named} attached images; a named pipe carries none, send them inline"
            ))),
        }
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        self.read_body::<InferEvent>()
            .await
            .map_err(|error| match error {
                TransportError::Malformed(why) => SessionError::Malformed(why),
                _ => SessionError::Closed,
            })
    }
}
