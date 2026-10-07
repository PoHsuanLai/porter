//! The socket carrier where there is none: no `socket` feature (no runtime, no latchkey), or no
//! Unix sockets. Nobody is reachable on it, and no session exists.

use crate::authenticated::Relayed;
use crate::env::{SocketAgent, StartAgent};
use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest};
#[cfg(feature = "infer")]
use porter_infer::{ClientFrame, InferEvent, LinkHello, SessionError};

/// A session that cannot exist: [`open`] never makes one.
#[cfg(feature = "infer")]
#[derive(Debug)]
pub(super) enum Link {}

#[cfg(feature = "infer")]
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

/// No address: nothing to resolve.
#[derive(Debug)]
pub(super) struct Door;

pub(super) fn door(_agent: &SocketAgent) -> Door {
    Door
}

pub(super) async fn reachable(_door: &Door, _how: &StartAgent) -> bool {
    false
}

pub(super) async fn call(
    _door: &Door,
    _how: &StartAgent,
    _request: AccountsRequest,
) -> Result<AccountsReply, TransportError> {
    Err(TransportError::Unreachable)
}

#[cfg(feature = "infer")]
pub(super) async fn open(
    _door: &Door,
    _how: &StartAgent,
    _hello: LinkHello,
) -> Result<Link, TransportError> {
    Err(TransportError::Unreachable)
}

pub(super) async fn open_authenticated(
    _door: &Door,
    _how: &StartAgent,
    _request: AccountsRequest,
) -> Result<Relayed, TransportError> {
    Err(TransportError::Unreachable)
}
