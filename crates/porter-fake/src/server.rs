//! The seam every fake network server implements. A fake server stands in for one real one (an
//! OAuth issuer, an IMAP host, a Nextcloud) on a scratch socket inside the jail; the real
//! provider file is read and its endpoints rewritten to reach it, so the code under test never
//! learns it is not talking to the real thing. Nothing here opens a network socket outside
//! the scratch directory or loopback.

use porter_provider::ProviderSpec;
use std::future::Future;
use std::path::PathBuf;

/// Where a fake server listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeAddress {
    /// A Unix socket in a scratch directory.
    Socket(PathBuf),
    /// A loopback port.
    Loopback(u16),
}

/// One fake server.
pub trait FakeServer: Send {
    /// Where it listens.
    fn address(&self) -> &FakeAddress;

    /// `spec` with the endpoints this server stands in for rewritten to reach it.
    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec;

    /// Serves until the future is dropped.
    fn serve(self) -> impl Future<Output = ()> + Send;
}
