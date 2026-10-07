//! The AgentLogin family (lane P1): an external coding agent that signs itself in (Claude Code,
//! Gemini CLI, Codex, any ACP agent; any file with `kind = "agent_login"`). The account holds NO
//! credential of any kind: the agent's own login lives in the agent's own state directory, which
//! porter never reads (design/31 R7, R8). Adding the account asks nothing; the person reviews the
//! agent and confirms, and the account is stored in `needs_login` until the launcher reports the
//! agent's state (`Peer.SetAgentState`).
//!
//! A session mints no token (there is nothing to mint it from) and revoking has nothing to do at a
//! provider: forgetting the account is all porter can do.

mod sign_in;

use porter_core::{
    Account, AccountId, Audience, Claim, Credential, IssuedToken, Offer, Provenance, Subject,
};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, RevokeOutcome, SignInStart,
};

pub use sign_in::AgentLoginSignIn;

/// The claims an agent file declares: one per program, about that program.
pub(crate) fn claims(spec: &ProviderSpec) -> Vec<Claim> {
    spec.capabilities
        .iter()
        .filter_map(|row| match &row.capability {
            porter_core::Capability::Agent(agent) => Some(Claim {
                subject: Subject::Agent(agent.program.clone()),
                offer: Offer::Present(row.capability.clone()),
                provenance: Provenance::Declared,
            }),
            _ => None,
        })
        .collect()
}

/// The AgentLogin family's provider.
#[derive(Debug, Clone)]
pub struct AgentLoginProvider {
    spec: ProviderSpec,
}

impl AgentLoginProvider {
    /// The provider serving the accounts of `spec`.
    pub fn new(spec: ProviderSpec) -> Self {
        Self { spec }
    }
}

/// An open agent account: it holds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentLoginSession;

impl Provider for AgentLoginProvider {
    type Session = AgentLoginSession;
    type SignIn = AgentLoginSignIn;

    fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    async fn discover(
        &self,
        _account: &Account,
        _presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        Ok(claims(&self.spec))
    }

    async fn open(
        &self,
        _account: &AccountId,
        presented: Presented,
    ) -> Result<AgentLoginSession, ProviderError> {
        // An agent account stores nothing, so it never presents a credential; one that did is
        // not an agent account's.
        match presented {
            Presented::Anonymous => Ok(AgentLoginSession),
            Presented::Credential(_) => Err(ProviderError::Unreadable),
        }
    }

    fn sign_in(&self, _start: SignInStart) -> Result<AgentLoginSignIn, ProviderError> {
        // Adding and signing in again are the same two steps: the agent holds the login either way.
        Ok(AgentLoginSignIn::new(self.spec.clone()))
    }

    async fn revoke(
        &self,
        _account: &Account,
        _presented: &Presented,
    ) -> Result<RevokeOutcome, ProviderError> {
        Ok(RevokeOutcome::Unsupported)
    }
}

impl ProviderSession for AgentLoginSession {
    async fn access_token(&self, _audience: &Audience) -> Result<IssuedToken, ProviderError> {
        // The agent's login is the agent's: there is nothing here to make a token from.
        Err(ProviderError::Forbidden)
    }

    fn renewed(&self) -> Option<Credential> {
        None
    }
}
