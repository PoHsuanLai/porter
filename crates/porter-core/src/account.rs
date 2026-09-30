//! One account as accountd holds it and as a granted app may read it.

use crate::auth_kind::AuthKind;
use crate::id::{AccountId, ProviderId};
use crate::offer::Claim;
use crate::restriction::Restriction;
use serde::{Deserialize, Serialize};

/// An account: a provider, a person's credential for it, and what it can do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Its id.
    pub id: AccountId,
    /// The provider file it was made from.
    pub provider: ProviderId,
    /// What the user reads (usually the address).
    pub label: AccountLabel,
    /// Whether it works right now.
    pub state: AccountState,
    /// How it signs in.
    pub auth: AuthKind,
    /// Its effective capabilities (`crate::effective`), one claim per subject and kind.
    pub capabilities: Vec<Claim>,
    /// What limits it.
    pub restriction: Restriction,
}

/// The name an account is shown by (`ada@example.org`, "Ollama on this computer").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountLabel(pub String);

/// Whether an account works right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountState {
    /// Signed in and reachable.
    Ok,
    /// The credential was refused; the user must sign in again (a revoked app password, C4).
    NeedsReauth,
    /// Not reachable (a stopped local runtime, no network). Never deleted for this.
    Offline,
    /// Working, with a restriction the UI explains.
    Limited,
}
