//! A consent decision and what it covers (design/31 §4.5).

use crate::app_id::AppId;
use crate::capability::CapabilityKind;
use crate::data_class::DataClass;
use crate::id::{AccountId, GrantId};
use crate::space::SpaceScope;
use crate::units::UnixSeconds;
use serde::{Deserialize, Serialize};

/// What one decision covers: this app, this account, this kind, this data class, this use.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GrantKey {
    /// The app.
    pub app: AppId,
    /// The account.
    pub account: AccountId,
    /// The capability kind.
    pub kind: CapabilityKind,
    /// The data class.
    pub class: DataClass,
    /// Interactive or background use.
    pub usage: Usage,
    /// The Spaces it covers: account grants made from the sheet cover `Any`.
    pub space: SpaceScope,
}

/// Whether the app acts for a person at the screen or on its own (indexing, backup).
/// Background use is a separate grant (design/31 §5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Usage {
    /// A person is waiting on the result.
    Interactive,
    /// Nobody is waiting.
    Background,
}

/// The user's answer. Ordered so that a denial outranks an allowance given at the same instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Allowed.
    Allow,
    /// Refused.
    Deny,
}

/// How long the answer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantScope {
    /// For one use; accountd drops it after that use. A use is the first of any kind: a token
    /// issued (`IssueToken`) or a relay opened (`OpenAuthenticated`, `OpenLinked`). Design/31
    /// §5.4 wrote "the first token issued" before relays existed; the first relay spends it too.
    Once,
    /// Until revoked in Settings.
    Always,
}

/// One stored consent decision, over any key type: accounts use [`GrantKey`], the action
/// router its own key (an action, a caller and a Space).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Grant<K = GrantKey> {
    /// Its id.
    pub id: GrantId,
    /// What it covers.
    pub key: K,
    /// The answer.
    pub decision: Decision,
    /// How long it holds.
    pub scope: GrantScope,
    /// When it was given.
    pub at: UnixSeconds,
}
