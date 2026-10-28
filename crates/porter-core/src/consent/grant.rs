//! A consent decision and what it covers (design/31 §4.5).

use crate::GrantId;
use crate::SessionToken;
use porter_ids::UnixSeconds;
use serde::{Deserialize, Serialize};

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

/// How long the answer holds. Stored as `"once"`, `"always"` or `{"session": "<id>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantScope {
    /// For one use; the service drops it after that use. A use is the first of any kind: a relay
    /// opened (`OpenAuthenticated`, `OpenLinked`) or a key read by the host that holds the
    /// secret.
    Once,
    /// Until revoked in Settings.
    Always,
    /// For one session of the asking app ("This session only"): from the answer until whoever
    /// hosts the session ends it. The service removes the grant then, and forgets it across a
    /// restart (sessions do not outlive the service). Only an ask made inside a session can have
    /// one made.
    Session(SessionToken),
}

impl GrantScope {
    /// The scope's word, which carries no session: `once`, `always`, `session`. A session
    /// travels beside it ([`GrantScope::session`]) where a scope is one string
    /// (`Peer.Verdicts`).
    pub fn word(&self) -> &'static str {
        match self {
            GrantScope::Once => "once",
            GrantScope::Always => "always",
            GrantScope::Session(_) => "session",
        }
    }

    /// The session a session scope lasts for.
    pub fn session(&self) -> Option<&SessionToken> {
        match self {
            GrantScope::Session(session) => Some(session),
            GrantScope::Once | GrantScope::Always => None,
        }
    }

    /// The scope a word and an optional session stand for: `session` needs its session and the
    /// other two take none.
    pub fn from_words(word: &str, session: Option<&SessionToken>) -> Option<Self> {
        match (word, session) {
            ("once", None) => Some(GrantScope::Once),
            ("always", None) => Some(GrantScope::Always),
            ("session", Some(session)) => Some(GrantScope::Session(session.clone())),
            _ => None,
        }
    }
}

/// One stored consent decision, over any key type: accounts use [`crate::GrantKey`], the action
/// router its own key (an action, a caller and a Space).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Grant<K = crate::GrantKey> {
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
