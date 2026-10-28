//! What accountd persists (porter PLAN §2.3): the registry as one JSON document. Once it is on
//! disk a vocabulary bump costs something: the stored `vocab` says which version wrote it, and
//! reading an older file goes through the migration table, one step per version that changed the
//! document's shape. A version that did not change it has no row: the document is read as it
//! was and only its version moves.

use crate::AccountId;
use crate::account::Account;
use crate::auth_kind::AuthKind;
use crate::consent::Grant;
use crate::identities::IdentityKind;
use porter_capability::Toggle;
use porter_capability::{CapabilityKind, VocabVersion};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;

/// The first vocabulary version a file was ever written with. Older numbers never reached a
/// disk, so there is nothing to migrate from.
pub const FIRST_PERSISTED: VocabVersion = VocabVersion(3);

/// Everything the registry keeps across restarts. Secrets are not here: they are in the
/// secret store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Persisted {
    /// The vocabulary version that wrote this document.
    pub vocab: VocabVersion,
    /// The accounts, with their effective capabilities and endpoints.
    pub accounts: Vec<Account>,
    /// The consent store.
    pub grants: Vec<Grant>,
    /// What the user turned off, per account and kind. Kinds without a row are on.
    pub toggles: Vec<AccountToggle>,
}

/// The user's switch for one kind of one account (Settings' per-service toggle).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountToggle {
    /// The account.
    pub account: AccountId,
    /// The kind.
    pub kind: CapabilityKind,
    /// On or off.
    pub toggle: Toggle,
}

/// Why a stored document was refused. accountd refuses to start over a file it cannot read
/// rather than start empty and overwrite it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StoreFault {
    /// Not valid JSON, or not the stored schema.
    #[error("registry file unreadable: {0}")]
    Unreadable(String),
    /// Written by a newer build.
    #[error("registry file is from vocabulary {0:?}, newer than this build")]
    FromNewerVocabulary(VocabVersion),
    /// Older than the first version ever stored, or a version whose change has no migration.
    #[error("no migration from vocabulary {0:?}")]
    NoMigration(VocabVersion),
    /// An account holds an endpoint porter would not use.
    #[error("account {0} holds an incoherent endpoint")]
    BadEndpoint(AccountId),
}

/// Why the registry could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The stored document was refused (corrupt, from a newer build); the daemon must not start
    /// empty over it.
    #[error(transparent)]
    Fault(#[from] StoreFault),
    /// The medium failed (a full disk, a read-only state directory).
    #[error("registry store unavailable")]
    Unavailable,
}

/// Where the registry lives between runs (porter PLAN §2.3): a seam, so the daemon writes a JSON
/// file atomically, an app hosting porter writes its platform state directory, and tests keep it
/// in memory. Loads and saves the registry as one document.
pub trait RegistryStore: Send + Sync {
    /// The stored registry; an empty one where nothing was stored yet.
    fn load(&self) -> impl Future<Output = Result<Persisted, StoreError>> + Send;

    /// Replaces the stored registry, all or nothing.
    fn save(&self, state: &Persisted) -> impl Future<Output = Result<(), StoreError>> + Send;
}

/// Stores nothing: the registry lives for the life of the process.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoStore;

impl RegistryStore for NoStore {
    async fn load(&self) -> Result<Persisted, StoreError> {
        Ok(Persisted::empty())
    }

    async fn save(&self, _state: &Persisted) -> Result<(), StoreError> {
        Ok(())
    }
}

/// A step from one vocabulary version to the next, over the document's JSON.
type Migration = fn(Value) -> Result<Value, StoreFault>;

/// Every step, keyed by the version it starts from. A bump of `VocabVersion::CURRENT` that changes
/// the stored document's shape adds its row here, with a fixture of the old document in the
/// tests; a bump that only adds defaulted fields or new variants adds nothing, since a document
/// written before it reads as it was. Versions 3 to 12 changed no stored shape, so the table
/// starts empty; their frozen documents are in the tests.
const MIGRATIONS: &[(VocabVersion, Migration)] = &[];

/// Drops from `persisted` what accountd no longer holds: it keeps the identity accounts (OAuth,
/// password, app password, and Nextcloud's sign-in, which is a password the server minted) and
/// the Tailscale account, and empties the grants and the toggles. Only accountd applies this,
/// and it keeps a copy of the old file first.
pub fn prune_to_identities(persisted: &mut Persisted) {
    persisted.accounts.retain(|account| {
        IdentityKind::of(account.auth).is_some()
            || matches!(account.auth, AuthKind::LoginFlowV2 | AuthKind::OwnProgram)
    });
    persisted.grants.clear();
    persisted.toggles.clear();
}

impl Persisted {
    /// A registry with nothing in it, at this build's vocabulary.
    pub fn empty() -> Self {
        Self {
            vocab: VocabVersion::CURRENT,
            accounts: Vec::new(),
            grants: Vec::new(),
            toggles: Vec::new(),
        }
    }

    /// The document written by this build.
    pub fn to_json(&self) -> Result<String, StoreFault> {
        serde_json::to_string_pretty(self).map_err(|e| StoreFault::Unreadable(e.to_string()))
    }

    /// The registry a stored document holds, migrated to this build's vocabulary.
    pub fn from_json(text: &str) -> Result<Self, StoreFault> {
        let document: Value =
            serde_json::from_str(text).map_err(|e| StoreFault::Unreadable(e.to_string()))?;
        let stored: VocabVersion = document
            .get("vocab")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .ok_or_else(|| StoreFault::Unreadable("no vocab".into()))?;
        let document = migrate(document, stored)?;
        let persisted: Persisted =
            serde_json::from_value(document).map_err(|e| StoreFault::Unreadable(e.to_string()))?;
        match persisted
            .accounts
            .iter()
            .find(|a| a.endpoints.iter().any(|e| e.check().is_err()))
        {
            Some(account) => Err(StoreFault::BadEndpoint(account.id.clone())),
            None => Ok(persisted),
        }
    }
}

/// `document`, written at `from`, brought to the current vocabulary one step at a time.
fn migrate(document: Value, from: VocabVersion) -> Result<Value, StoreFault> {
    let current = VocabVersion::CURRENT;
    if from > current {
        return Err(StoreFault::FromNewerVocabulary(from));
    }
    if from < FIRST_PERSISTED {
        return Err(StoreFault::NoMigration(from));
    }
    let mut at = from;
    let mut document = document;
    while at < current {
        if let Some((_, step)) = MIGRATIONS.iter().find(|(start, _)| *start == at) {
            document = step(document)?;
        }
        at = VocabVersion(at.0 + 1);
        if let Some(slot) = document.get_mut("vocab") {
            *slot = Value::from(at.0);
        }
    }
    Ok(document)
}

#[cfg(test)]
mod tests;
