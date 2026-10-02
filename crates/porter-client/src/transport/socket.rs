//! The latchkey socket carrier: `porter_core::wire` frames over a Unix socket or a Windows
//! named pipe, for other desktops, macOS and Windows. Frozen shape; not built yet.

use super::Transport;
use crate::env::SocketPath;
use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest, DataClass, Need, Tier};
use porter_infer::{ClientFrame, InferEvent, InferSession, SessionError};

/// A connection to the agent on the latchkey socket.
#[derive(Debug)]
pub struct SocketTransport {
    path: SocketPath,
}

impl SocketTransport {
    /// A transport to the agent at `path` (connected on first use).
    pub fn at(path: SocketPath) -> Self {
        Self { path }
    }
}

/// A streaming session on inferd's socket.
#[derive(Debug)]
pub struct SocketSession {
    _private: (),
}

impl InferSession for SocketSession {
    async fn send(&mut self, _frame: ClientFrame) -> Result<(), SessionError> {
        todo!("encode_frame the ClientFrame onto the socket")
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        todo!("decode_frame the next InferEvent from the socket")
    }
}

impl Transport for SocketTransport {
    type Session = SocketSession;

    async fn call(&self, _request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        let _ = &self.path;
        todo!("latchkey connect, encode_frame the request, decode_frame the reply")
    }

    async fn open(
        &self,
        _need: &Need,
        _class: DataClass,
        _tier: Tier,
    ) -> Result<SocketSession, TransportError> {
        todo!("the same framing to inferd's socket, one connection per session")
    }
}
