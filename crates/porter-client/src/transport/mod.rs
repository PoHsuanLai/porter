//! The carrier seam: one request in, one reply out. The caller's identity is the transport's
//! to establish, never the request's.

#[cfg(feature = "dbus")]
mod dbus;
mod in_process;
mod socket;

#[cfg(feature = "dbus")]
pub use dbus::DbusTransport;
pub use in_process::InProcess;
pub use socket::SocketTransport;

use crate::error::TransportError;
use porter_core::{AccountsReply, AccountsRequest};
use porter_infer::{InferReply, InferRequest};
use std::future::Future;

/// Carries requests to accountd and inferd.
pub trait Transport: Send + Sync {
    /// One request to accountd.
    fn call(
        &self,
        request: AccountsRequest,
    ) -> impl Future<Output = Result<AccountsReply, TransportError>> + Send;

    /// One request to inferd.
    fn infer(
        &self,
        request: InferRequest,
    ) -> impl Future<Output = Result<InferReply, TransportError>> + Send;
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

impl Transport for AnyTransport {
    async fn call(&self, request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        match self {
            #[cfg(feature = "dbus")]
            AnyTransport::Dbus(link) => link.call(request).await,
            AnyTransport::Socket(link) => link.call(request).await,
        }
    }

    async fn infer(&self, request: InferRequest) -> Result<InferReply, TransportError> {
        match self {
            #[cfg(feature = "dbus")]
            AnyTransport::Dbus(link) => link.infer(request).await,
            AnyTransport::Socket(link) => link.infer(request).await,
        }
    }
}
