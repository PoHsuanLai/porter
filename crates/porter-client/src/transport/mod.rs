//! The carrier seam: one request in, one reply out. The caller's identity is the transport's
//! to establish, never the request's.

#[cfg(feature = "dbus")]
mod dbus;
#[cfg(feature = "dbus")]
mod dbus_session;
mod in_process;
mod socket;

#[cfg(feature = "dbus")]
pub use dbus::DbusTransport;
#[cfg(feature = "dbus")]
pub use dbus_session::{DbusSession, MAX_ATTACHMENTS};
pub use in_process::{InProcess, InProcessSession};
pub use socket::{SocketSession, SocketTransport};

use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest, DataClass, Need, Tier};
use porter_infer::{ClientFrame, InferEvent, InferSession, OpenOptions, SessionError};
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
    /// (`Finished(Refused(..))`). `options` carries the caller's `traceparent` (the bus
    /// `options` dictionary, the socket's first frame) so one task is one trace.
    fn open_with(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> impl Future<Output = Result<Self::Session, TransportError>> + Send;

    /// [`Transport::open_with`] with no trace context: inferd starts its own root.
    fn open(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
    ) -> impl Future<Output = Result<Self::Session, TransportError>> + Send {
        async move {
            self.open_with(need, class, tier, &OpenOptions { traceparent: None })
                .await
        }
    }
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

    #[cfg(all(unix, feature = "dbus"))]
    async fn send_attached(
        &mut self,
        frame: ClientFrame,
        attachments: Vec<std::os::fd::OwnedFd>,
    ) -> Result<(), SessionError> {
        match self {
            AnySession::Dbus(session) => session.send_attached(frame, attachments).await,
            AnySession::Socket(session) => session.send_attached(frame, attachments).await,
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

    async fn open_with(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<AnySession, TransportError> {
        match self {
            #[cfg(feature = "dbus")]
            AnyTransport::Dbus(link) => link
                .open_with(need, class, tier, options)
                .await
                .map(AnySession::Dbus),
            AnyTransport::Socket(link) => link
                .open_with(need, class, tier, options)
                .await
                .map(AnySession::Socket),
        }
    }
}
