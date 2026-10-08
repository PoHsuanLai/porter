//! `org.quire.Accounts1.Tokens` at `/org/quire/Accounts1`: short-lived access only; refresh
//! tokens, passwords and keys never cross the bus (design/31 §4.6).

use crate::args::TokenArg;
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedFd, OwnedValue};

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
    /// An API key for a process the agent launcher spawns (agent-session ask P2): `AgentLauncher`
    /// only, and only for a program the caller's connection has registered. `program` is the
    /// agent program, `target` is `memfd` or `tmpfs_file` (`Handoff`). The reply is the
    /// credential's id and its handle in a variant: a file descriptor (`h`, a sealed memfd) for
    /// `memfd`, the path (`s`) of a 0600 file for `tmpfs_file`. The key is in neither the body
    /// nor any signal.
    fn issue_process_credential(
        &self,
        grant: &str,
        program: &str,
        target: &str,
    ) -> zbus::Result<(String, OwnedValue)>;
    /// Ends a credential this connection was issued: a tmpfs file is unlinked, and the launcher
    /// ends the process (a memfd that was passed on cannot be recalled).
    fn revoke_process_credential(&self, id: &str) -> zbus::Result<()>;
    /// Sent to the launcher that was issued the credential, alone: accountd ended it (the grant
    /// was revoked or the account removed), so the launcher must end the process. `reason` is a
    /// `CredentialEnd` word. Not sent when the launcher itself ended it or left the bus.
    #[zbus(signal)]
    fn process_credential_revoked(&self, id: String, reason: String) -> zbus::Result<()>;
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

    fn issue_process_credential(
        &self,
        grant: String,
        program: String,
        target: String,
    ) -> fdo::Result<(String, OwnedValue)> {
        let _ = (grant, program, target);
        Err(crate::introspect::frozen())
    }

    fn revoke_process_credential(&self, id: String) -> fdo::Result<()> {
        let _ = id;
        Err(crate::introspect::frozen())
    }

    #[zbus(signal)]
    async fn process_credential_revoked(
        emitter: &SignalEmitter<'_>,
        id: &str,
        reason: &str,
    ) -> zbus::Result<()>;
}
