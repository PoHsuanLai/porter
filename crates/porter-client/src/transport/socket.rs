//! The latchkey socket carrier: `porter_core::wire` frames over a Unix socket or a Windows
//! named pipe, for other desktops, macOS and Windows. Frozen shape; not built yet.

use super::Transport;
use crate::env::SocketPath;
use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest};
use porter_infer::{InferReply, InferRequest};

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

impl Transport for SocketTransport {
    async fn call(&self, _request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        let _ = &self.path;
        todo!("latchkey connect, encode_frame the request, decode_frame the reply")
    }

    async fn infer(&self, _request: InferRequest) -> Result<InferReply, TransportError> {
        todo!("the same frames to inferd's socket")
    }
}
