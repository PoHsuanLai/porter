//! What accountd persists (porter PLAN §2.3): the registry as one JSON document. Once it is on
//! disk a vocabulary bump costs something: the stored `vocab` says which version wrote it, and
//! reading an older file goes through the migration table, one step per version.

use crate::account::Account;
use crate::capability::{CapabilityKind, VocabVersion};
use crate::consent::Grant;
use crate::effective::Toggle;
use crate::id::AccountId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
pub enum StoreFault {
    /// Not valid JSON, or not the stored schema.
    #[error("registry file unreadable: {0}")]
    Unreadable(String),
    /// Written by a newer build.
    #[error("registry file is from vocabulary {0:?}, newer than this build")]
    FromNewerVocabulary(VocabVersion),
    /// Older than any version with a migration.
    #[error("no migration from vocabulary {0:?}")]
    NoMigration(VocabVersion),
    /// An account holds an endpoint porter would not use.
    #[error("account {0} holds an incoherent endpoint")]
    BadEndpoint(AccountId),
}

/// A step from one vocabulary version to the next, over the document's JSON.
type Migration = fn(Value) -> Result<Value, StoreFault>;

/// Every step, keyed by the version it starts from. A bump of `VocabVersion::CURRENT` adds its
/// row here, with a fixture of the old document in the tests.
const MIGRATIONS: &[(VocabVersion, Migration)] = &[
    (VocabVersion(3), from_three),
    (VocabVersion(4), from_four),
    (VocabVersion(5), from_five),
    (VocabVersion(6), from_six),
    (VocabVersion(7), from_seven),
    (VocabVersion(8), from_eight),
];

/// 3 to 4 added the `sieve` family and changed the sheet's wire types; the stored document is
/// the same shape, so only its version moves.
fn from_three(document: Value) -> Result<Value, StoreFault> {
    Ok(document)
}

/// 4 to 5 added the `agent` capability kind, `Subject::Agent`, `AuthKind::AgentLogin` and
/// `AccountState::NeedsLogin`, all new variants: a document written at 4 has none of them and
/// reads as it was, so only its version moves.
fn from_four(document: Value) -> Result<Value, StoreFault> {
    Ok(document)
}

/// 5 to 6 added the `tasks` data class (a grant for Task lists): a new variant, so a document
/// written at 5 has none and reads as it was, and only its version moves.
fn from_five(document: Value) -> Result<Value, StoreFault> {
    Ok(document)
}

/// 6 to 7 added `Restriction::signed_in` (defaulted, absent when unknown), `ReauthReason`,
/// `ProviderKind` and the `row` of the sheet's views, all defaulted: a document written at 6
/// has none and reads as it was, with the sign-in's age unknown, so only its version moves.
fn from_six(document: Value) -> Result<Value, StoreFault> {
    Ok(document)
}

/// 7 to 8 added `GrantScope::Session` (`{"session": "<id>"}` beside `"once"` and `"always"`), a
/// new variant: a document written at 7 holds only the two old words and reads as it was, so
/// only its version moves.
fn from_seven(document: Value) -> Result<Value, StoreFault> {
    Ok(document)
}

/// 8 to 9 added `SignInFault::AlreadyAdded` and the defaulted `app_label` of `ConsentAsk` and
/// `allow_label` of `ReviewView`, all of the sheet's wire and none of the document: a document
/// written at 8 reads as it was, so only its version moves.
fn from_eight(document: Value) -> Result<Value, StoreFault> {
    Ok(document)
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
        let step = MIGRATIONS
            .iter()
            .find(|(start, _)| *start == at)
            .map(|(_, step)| step)
            .ok_or(StoreFault::NoMigration(at))?;
        document = step(document)?;
        at = VocabVersion(at.0 + 1);
        if let Some(slot) = document.get_mut("vocab") {
            *slot = Value::from(at.0);
        }
    }
    Ok(document)
}

#[cfg(test)]
mod tests;
