//! One account as accountd holds it and as a granted app may read it.

use crate::auth_kind::AuthKind;
use crate::endpoint::ServiceEndpoint;
use crate::id::{AccountId, ProviderId};
use crate::offer::Claim;
use crate::restriction::Restriction;
use serde::{Deserialize, Serialize};

/// An account: a provider, a person's credential for it, what it can do and where it is.
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
    /// Its servers: what mailo dials for mail, what syncd reads for files. Found at sign-in
    /// (autoconfig, Login Flow v2's `server`) and stored with the account; none is secret.
    pub endpoints: Vec<ServiceEndpoint>,
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
    /// A program that holds its own sign-in has not signed in (or says it is signed out): an
    /// agent, or Tailscale. The person signs in inside that program, never by giving porter a
    /// password. Only `AgentLogin` and `OwnProgram` accounts are ever here.
    NeedsLogin,
}

impl AccountState {
    /// Whether the person has to do something before the account works again: sign it in again
    /// (`NeedsReauth`) or sign the agent in (`NeedsLogin`). The shell hears both the same way.
    pub fn needs_person(self) -> bool {
        matches!(self, AccountState::NeedsReauth | AccountState::NeedsLogin)
    }
}

/// What an agent program says of its own sign-in, as the launcher reports it. It is the only
/// thing porter holds for an `AgentLogin` account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// The agent says it is signed in (a session started, or its `authenticate` succeeded).
    Ready,
    /// The agent says it needs a login.
    NeedsLogin,
}

impl AgentState {
    /// The state an account in this agent state has.
    pub fn account_state(self) -> AccountState {
        match self {
            AgentState::Ready => AccountState::Ok,
            AgentState::NeedsLogin => AccountState::NeedsLogin,
        }
    }
}
