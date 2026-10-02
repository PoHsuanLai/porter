//! The carrier seam: one request in, one reply out. The caller's identity is the transport's
//! to establish, never the request's.

#[cfg(feature = "dbus")]
mod dbus;
mod in_process;
mod socket;

#[cfg(feature = "dbus")]
pub use dbus::{DbusSession, DbusTransport};
pub use in_process::{InProcess, InProcessSession};
pub use socket::{SocketSession, SocketTransport};

use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest, DataClass, Need, Tier};
use porter_infer::{ClientFrame, InferEvent, InferSession, SessionError};
use std::future::Future;

/// Carries requests to accountd and inferd.
pub trait Transport: Send + Sync {
    /// The streaming session `open` returns.
    type Session: InferSession;

    /// One request to accountd.
    fn call(
        &self,
        request: AccountsRequest,
    ) -> impl Future<Output = Result<AccountsReply, TransportError>> + Send;

    /// Opens a session with inferd for `need`, `class` and `tier`: the route is chosen once, so
    /// the session is pinned to one model. A refusal arrives as the session's first event
    /// (`Finished(Refused(..))`).
    fn open(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
    ) -> impl Future<Output = Result<Self::Session, TransportError>> + Send;
}

/// The daemon links `Accounts::connect` picks between.
#[derive(Debug)]
pub enum AnyTransport {
    /// accountd on the session bus.
    #[cfg(feature = "dbus")]
    Dbus(DbusTransport),
    /// accountd on the latchkey socket.
    Socket(SocketTransport),
}

/// The sessions of [`AnyTransport`].
#[derive(Debug)]
pub enum AnySession {
    /// Over inferd on the session bus.
    #[cfg(feature = "dbus")]
    Dbus(DbusSession),
    /// Over inferd's socket.
    Socket(SocketSession),
}

impl InferSession for AnySession {
    async fn send(&mut self, frame: ClientFrame) -> Result<(), SessionError> {
        match self {
            #[cfg(feature = "dbus")]
            AnySession::Dbus(session) => session.send(frame).await,
            AnySession::Socket(session) => session.send(frame).await,
        }
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        match self {
            #[cfg(feature = "dbus")]
            AnySession::Dbus(session) => session.next().await,
            AnySession::Socket(session) => session.next().await,
        }
    }
}

impl Transport for AnyTransport {
    type Session = AnySession;

    async fn call(&self, request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        match self {
            #[cfg(feature = "dbus")]
            AnyTransport::Dbus(link) => link.call(request).await,
            AnyTransport::Socket(link) => link.call(request).await,
        }
    }

    async fn open(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
    ) -> Result<AnySession, TransportError> {
        match self {
            #[cfg(feature = "dbus")]
            AnyTransport::Dbus(link) => link.open(need, class, tier).await.map(AnySession::Dbus),
            AnyTransport::Socket(link) => {
                link.open(need, class, tier).await.map(AnySession::Socket)
            }
        }
    }
}
