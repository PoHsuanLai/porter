//! `org.quire.Accounts1.Tokens` at `/org/quire/Accounts1`: short-lived access only; refresh
//! tokens, passwords and keys never cross the bus (design/31 §4.6).

use crate::args::TokenArg;
use zbus::fdo;
use zbus::zvariant::OwnedFd;

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Accounts1.Tokens",
    default_service = "org.quire.Accounts1",
    default_path = "/org/quire/Accounts1"
)]
pub trait Tokens {
    /// A token for a granted account: `bearer`, `xoauth2` or `api_key_handle`.
    fn issue_token(&self, grant: &str, audience: &str) -> zbus::Result<TokenArg>;
    /// A socket to a daemon-side authenticated proxy (IMAP LOGIN, WebDAV basic), for
    /// password protocols without releasing the password.
    fn open_authenticated(&self, grant: &str, endpoint: &str) -> zbus::Result<OwnedFd>;
    /// A socket to a relay that adds no credential, to an origin the account's provider file
    /// declares for the grant's kind (a pre-authenticated link's host, Graph's `uploadUrl`).
    fn open_linked(&self, grant: &str, origin: &str) -> zbus::Result<OwnedFd>;
}

/// The daemon's side.
#[derive(Debug, Default)]
pub struct TokensSkeleton;

#[zbus::interface(name = "org.quire.Accounts1.Tokens")]
impl TokensSkeleton {
    fn issue_token(&self, grant: String, audience: String) -> fdo::Result<TokenArg> {
        let _ = (grant, audience);
        Err(crate::introspect::frozen())
    }

    fn open_authenticated(&self, grant: String, endpoint: String) -> fdo::Result<OwnedFd> {
        let _ = (grant, endpoint);
        Err(crate::introspect::frozen())
    }

    fn open_linked(&self, grant: String, origin: String) -> fdo::Result<OwnedFd> {
        let _ = (grant, origin);
        Err(crate::introspect::frozen())
    }
}
