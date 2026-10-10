//! `Peer.ResolveKey`'s two parts: the desk that reads an account's API key and notes that one was
//! released (or handed to a spawned process, `Tokens.IssueProcessCredential`), and the sealed
//! memfd the key travels on.
//!
//! The desk is a seam of accountd's own rather than a method of the service, because the key is
//! read with the same `Secrets` the service files it with and only a porter daemon may ask. An
//! entry of the audit file says which grant a key was released under and to whom, never the key.

use porter_core::audit::{AuditEntry, AuditEvent, CredentialEnd, Handoff};
use porter_core::wire::Refusal;
use porter_core::{
    Account, AccountId, AccountState, AppId, Audience, CapabilityKind, Credential, GrantId,
    SecretKey, SecretPurpose, SecretText, Toggle,
};
use porter_secrets::{Secrets, SecretsError};
use porter_service::{AuditSink, Clock, Registry};
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
use std::future::Future;
use std::io::{self, Seek, Write};
use std::os::fd::OwnedFd;
use std::pin::Pin;

/// A boxed future, so the desk can be used as a trait object.
pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Reads API keys for `Peer.ResolveKey` and notes each release.
pub trait KeyDesk: std::fmt::Debug + Send + Sync + 'static {
    /// The API key filed for `account`. A key that is gone or unreadable means signing in again.
    fn read<'a>(&'a self, account: &'a AccountId) -> Boxed<'a, Result<SecretText, Refusal>>;

    /// Records that the key of `account` was released under `grant`, resolved for `audience`
    /// (the app the grant is for: `org.quire.Agent.<program>` on an agent route).
    fn note(&self, audience: &AppId, account: &AccountId, grant: &GrantId);

    /// Records that the key of `account` was handed to a process for `audience` (the app the grant
    /// is for), by `handoff` (`Tokens.IssueProcessCredential`).
    fn note_handoff(&self, audience: &AppId, account: &AccountId, handoff: Handoff);

    /// Records that a credential handed to a process for `audience` ended, and why.
    fn note_handoff_end(&self, audience: &AppId, account: &AccountId, reason: CredentialEnd);
}

/// The desk over the secret store, an audit sink and a clock.
#[derive(Debug, Clone)]
pub struct SecretsDesk<S, A, K> {
    secrets: S,
    audit: A,
    clock: K,
}

impl<S, A, K> SecretsDesk<S, A, K> {
    /// A desk reading from `secrets` and recording to `audit`, dated by `clock`.
    pub fn new(secrets: S, audit: A, clock: K) -> Self {
        Self {
            secrets,
            audit,
            clock,
        }
    }
}

/// What the store's failure is to the daemon that asked.
fn refusal_of(error: SecretsError) -> Refusal {
    match error {
        SecretsError::Missing | SecretsError::Unreadable => Refusal::NeedsReauth,
        SecretsError::Locked | SecretsError::Unavailable => Refusal::Unavailable,
    }
}

impl<S, A, K> KeyDesk for SecretsDesk<S, A, K>
where
    S: Secrets + std::fmt::Debug + 'static,
    A: AuditSink + std::fmt::Debug + 'static,
    K: Clock + std::fmt::Debug + 'static,
{
    fn read<'a>(&'a self, account: &'a AccountId) -> Boxed<'a, Result<SecretText, Refusal>> {
        Box::pin(async move {
            let key = SecretKey {
                account: account.clone(),
                purpose: SecretPurpose::ApiKey,
            };
            match self.secrets.get(&key).await.map_err(refusal_of)? {
                Credential::ApiKey(key) => Ok(key),
                _ => Err(Refusal::NeedsReauth),
            }
        })
    }

    fn note(&self, audience: &AppId, account: &AccountId, grant: &GrantId) {
        self.audit.record(AuditEntry::new(
            self.clock.now(),
            Some(audience.clone()),
            Some(account.clone()),
            AuditEvent::KeyResolved {
                grant: grant.clone(),
                audience: Audience(audience.name.as_str().to_owned()),
            },
        ));
    }

    fn note_handoff(&self, audience: &AppId, account: &AccountId, handoff: Handoff) {
        self.audit.record(AuditEntry::new(
            self.clock.now(),
            Some(audience.clone()),
            Some(account.clone()),
            AuditEvent::ProcessCredentialIssued {
                audience: Audience(audience.name.as_str().to_owned()),
                handoff,
            },
        ));
    }

    fn note_handoff_end(&self, audience: &AppId, account: &AccountId, reason: CredentialEnd) {
        self.audit.record(AuditEntry::new(
            self.clock.now(),
            Some(audience.clone()),
            Some(account.clone()),
            AuditEvent::ProcessCredentialRevoked {
                audience: Audience(audience.name.as_str().to_owned()),
                reason,
            },
        ));
    }
}

/// Whether the key of `account` may leave now: not when the person turned the account off for Llm
/// (`Denied`) nor while it waits to be signed in again (`NeedsReauth`). Shared by
/// `Peer.ResolveKey` and `Tokens.IssueProcessCredential`.
pub(crate) fn usable(registry: &Registry, account: &Account) -> Result<(), Refusal> {
    let off = registry.toggles.iter().any(|t| {
        t.account == account.id && t.kind == CapabilityKind::Llm && t.toggle == Toggle::Off
    });
    match (off, account.state) {
        (true, _) => Err(Refusal::Denied),
        (false, AccountState::NeedsReauth) => Err(Refusal::NeedsReauth),
        (false, _) => Ok(()),
    }
}

/// `key` on an anonymous file, read position at the start, sealed against every change: no write,
/// no growing, no shrinking, and no seal added or removed. A reader gets the key and cannot alter
/// what another reader of the same file sees.
pub fn sealed_key(key: &str) -> io::Result<OwnedFd> {
    let fd = memfd_create(
        "porter-api-key",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )?;
    let mut file = std::fs::File::from(fd);
    file.write_all(key.as_bytes())?;
    file.rewind()?;
    let fd = OwnedFd::from(file);
    fcntl_add_seals(
        &fd,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )?;
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::fs::{fcntl_get_seals, ftruncate};
    use std::io::Read;

    #[test]
    fn a_sealed_key_reads_back_and_cannot_change() {
        let fd = sealed_key("sk-or-v1-abc").expect("memfd");
        assert_eq!(
            fcntl_get_seals(&fd).expect("seals"),
            SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL
        );
        let mut file = std::fs::File::from(fd);
        let mut text = String::new();
        file.read_to_string(&mut text).expect("read");
        assert_eq!(text, "sk-or-v1-abc");
        // Every way of altering it, and of lifting a seal, is refused.
        assert!(file.write_all(b"x").is_err());
        assert!(ftruncate(&file, 0).is_err());
        assert!(ftruncate(&file, 4096).is_err());
        assert!(fcntl_add_seals(&file, SealFlags::empty()).is_err());
        file.rewind().expect("rewind");
        let mut again = String::new();
        file.read_to_string(&mut again).expect("read again");
        assert_eq!(again, text);
    }
}
