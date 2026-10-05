//! The network the host passes in: dial the endpoint, and upgrade to TLS. The relay never opens
//! a socket or builds a TLS configuration itself, so the same machines run against a real host
//! and a fake server with a scratch CA.

use porter_core::stream::ByteStream;
use porter_core::{Origin, Tls};
use std::future::Future;

/// Why a connection could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectFault {
    /// No route, refused, or the name does not resolve.
    Unreachable,
    /// The handshake failed or the certificate was refused for the server's name.
    Tls,
}

/// Opens connections to an endpoint's origin.
pub trait Connect: Send + Sync {
    /// The stream a connection is.
    type Stream: ByteStream;

    /// Connects to `origin`. With `Tls::Implicit` the connection is TLS from the first byte,
    /// checked against the origin's host; with `StartTls` or `Plain` it is plain until
    /// [`Connect::upgrade`].
    fn dial(
        &self,
        origin: &Origin,
        tls: Tls,
    ) -> impl Future<Output = Result<Self::Stream, ConnectFault>> + Send;

    /// Upgrades a plain connection to TLS after `STARTTLS` was accepted, checking the server's
    /// certificate for `host`.
    fn upgrade(
        &self,
        stream: Self::Stream,
        host: &str,
    ) -> impl Future<Output = Result<Self::Stream, ConnectFault>> + Send;
}
