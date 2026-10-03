//! The socket carrier where there is none: no `socket` feature (no runtime), or no Unix
//! sockets. Nobody is reachable on it, and no session exists.

use crate::env::SocketPath;
use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest};
use porter_infer::{ClientFrame, InferEvent, LinkHello, SessionError};

/// A session that cannot exist: [`open`] never makes one.
#[derive(Debug)]
pub(super) enum Link {}

impl Link {
    pub(super) async fn send(&mut self, _frame: ClientFrame) -> Result<(), SessionError> {
        match *self {}
    }

    #[cfg(unix)]
    pub(super) async fn send_attached(
        &mut self,
        _frame: ClientFrame,
        _attachments: Vec<std::os::fd::OwnedFd>,
    ) -> Result<(), SessionError> {
        match *self {}
    }

    pub(super) async fn next(&mut self) -> Result<InferEvent, SessionError> {
        match *self {}
    }
}

pub(super) async fn reachable(_path: &SocketPath) -> bool {
    false
}

pub(super) async fn call(
    _path: &SocketPath,
    _request: AccountsRequest,
) -> Result<AccountsReply, TransportError> {
    Err(TransportError::Unreachable)
}

pub(super) async fn open(_path: &SocketPath, _hello: LinkHello) -> Result<Link, TransportError> {
    Err(TransportError::Unreachable)
}
