//! The seam every fake network server implements. A fake server stands in for one real one (an
//! OAuth issuer, an IMAP host, a Nextcloud) on a scratch socket inside the jail; the real
//! provider file is read and its endpoints rewritten to reach it, so the code under test never
//! learns it is not talking to the real thing. Nothing here opens a network socket outside
//! the scratch directory or loopback.

use porter_provider::ProviderSpec;
use std::future::Future;
use std::path::PathBuf;

/// What a fake server speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FakeProtocol {
    /// An OAuth issuer: authorize, token, device code, revoke, with PKCE and rotation.
    OAuthIssuer,
    /// An IMAP server: LOGIN, AUTHENTICATE PLAIN and XOAUTH2, STARTTLS with a scratch CA.
    Imap,
    /// An SMTP submission server: EHLO, STARTTLS, AUTH.
    Smtp,
    /// A POP3 server: CAPA, STLS, USER/PASS, AUTH PLAIN and XOAUTH2.
    Pop3,
    /// A Nextcloud: Login Flow v2, OCS capabilities, DAV with a sync token, quota, notes.
    Nextcloud,
    /// A plain DAV server.
    Dav,
    /// A Microsoft Graph drive.
    Graph,
    /// An autoconfig and DNS answer.
    Autoconfig,
    /// A model list: Ollama's, or an OpenAI-compatible one.
    ModelList,
}

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
    /// What it speaks.
    fn protocol(&self) -> FakeProtocol;

    /// Where it listens.
    fn address(&self) -> &FakeAddress;

    /// `spec` with the endpoints this server stands in for rewritten to reach it.
    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec;

    /// Serves until the future is dropped.
    fn serve(self) -> impl Future<Output = ()> + Send;
}
