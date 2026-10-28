//! The network the host passes in: dial the endpoint, and upgrade to TLS. The relay never opens
//! a socket or builds a TLS configuration itself, so the same machines run against a real host
//! and a fake server with a scratch CA.

use crate::fault::RelayFault;
use porter_core::Tls;
use porter_core::stream::ByteStream;
use porter_ids::Origin;
use std::future::Future;

/// Opens connections to an endpoint's origin. A connection that cannot be made is
/// [`RelayFault::Unreachable`] (no route, refused, or the name does not resolve) or
/// [`RelayFault::Tls`] (the handshake failed or the certificate was refused for the server's
/// name).
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
    ) -> impl Future<Output = Result<Self::Stream, RelayFault>> + Send;

    /// Upgrades a plain connection to TLS after `STARTTLS` was accepted, checking the server's
    /// certificate for `host`.
    fn upgrade(
        &self,
        stream: Self::Stream,
        host: &str,
    ) -> impl Future<Output = Result<Self::Stream, RelayFault>> + Send;
}
