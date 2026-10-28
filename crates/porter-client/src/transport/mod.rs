//! The carrier seam: one request in, one reply out. The caller's identity is the transport's
//! to establish, never the request's. Inference is `inferd-client`'s.

#[cfg(feature = "dbus")]
mod dbus;
#[cfg(feature = "dbus")]
mod dbus_accounts;
#[cfg(feature = "in-process")]
mod in_process;

#[cfg(feature = "dbus")]
pub use dbus::DbusTransport;
#[cfg(feature = "in-process")]
pub use in_process::{HostedCore, InProcess};

use crate::authenticated::Relayed;
use crate::error::TransportError;
use porter_core::EndpointUrl;
use porter_core::GrantId;
use porter_wire::{AccountsReply, AccountsRequest};
use std::future::Future;

/// Carries requests to accountd.
pub trait Transport: Send + Sync {
    /// One request to accountd.
    fn call(
        &self,
        request: AccountsRequest,
    ) -> impl Future<Output = Result<AccountsReply, TransportError>> + Send;

    /// A byte stream to one endpoint of a granted account, authenticated by a daemon-side relay
    /// (`Tokens.OpenAuthenticated`). The descriptor is out of band, so it is not a `call`. A
    /// transport that cannot carry one is `Unreachable`.
    fn open_authenticated(
        &self,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> impl Future<Output = Result<Relayed, TransportError>> + Send {
        let _ = (grant, endpoint);
        async { Err(TransportError::Unreachable) }
    }

    /// A byte stream to `origin`, a host the account's provider file declares for the grant's
    /// kind as one its pre-authenticated links point at (`Tokens.OpenLinked`); the relay adds no
    /// credential. The descriptor is out of band. A transport that cannot carry one is
    /// `Unreachable`.
    fn open_linked(
        &self,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> impl Future<Output = Result<Relayed, TransportError>> + Send {
        let _ = (grant, origin);
        async { Err(TransportError::Unreachable) }
    }
}
