//! Reading an AI key out of inferd's own secret store, and noting that it was read.
//!
//! The key never leaves this process: the cloud turn asks for it, uses it for one request and
//! drops it. The desk is a seam of its own rather than a method of the service because the key is
//! read with the same `Secrets` the service files it with. One line of the audit file says which
//! permit a key was used under and by whom, never the key.

use crate::cloud::accountd::Boxed;
use porter_capability::{CapabilityKind, Toggle};
use porter_core::{
    Account, AccountState, AppId, Audience, Credential, SecretKey, SecretPurpose, SecretText,
};
use porter_core::{AccountId, GrantId};
use porter_ids::clock::Clock;
use porter_secrets::{Secrets, SecretsError};
use porter_service::audit::{AuditEntry, AuditEvent};
use porter_service::{AuditSink, Registry};
use porter_wire::Refusal;

/// Reads API keys and notes each use.
pub trait KeyDesk: std::fmt::Debug + Send + Sync + 'static {
    /// The API key filed for `account`. A key that is gone or unreadable means signing in again.
    fn read<'a>(&'a self, account: &'a AccountId) -> Boxed<'a, Result<SecretText, Refusal>>;

    /// Records that the key of `account` was used under `grant` for `audience` (the app the
    /// permit is for).
    fn note(&self, audience: &AppId, account: &AccountId, grant: &GrantId);
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

/// What the store's failure is to the one who asked.
fn refusal_of(error: SecretsError) -> Refusal {
    match error {
        SecretsError::Missing | SecretsError::Unreadable => Refusal::NeedsReauth,
        _ => Refusal::Unavailable,
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
}

/// Whether the key of `account` may be used now: not when the person turned the account off for
/// language models (`Denied`) nor while it waits to be signed in again (`NeedsReauth`).
pub fn usable(registry: &Registry, account: &Account) -> Result<(), Refusal> {
    let off = registry.toggles.iter().any(|t| {
        t.account == account.id && t.kind == CapabilityKind::Llm && t.toggle == Toggle::Off
    });
    match (off, account.state) {
        (true, _) => Err(Refusal::Denied),
        (false, AccountState::NeedsReauth) => Err(Refusal::NeedsReauth),
        (false, _) => Ok(()),
    }
}
