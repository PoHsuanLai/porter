//! The Tailnet family: Tailscale on this computer (any file with `kind = "own_program"`). The
//! account holds NO credential of any kind: Tailscale's own program keeps the login and porter
//! only asks it, over the socket it serves, whether it is running and who is signed in
//! (`porter-tailscale`). Adding the account is asking: it is added when Tailscale answers and
//! somebody is signed in, and when nobody is, the sign-in hands the person Tailscale's own page
//! to open. porter never takes a Tailscale password or key.
//!
//! A session mints no token (there is nothing to mint it from) and revoking has nothing to do at
//! a provider: forgetting the account does not sign Tailscale out.

mod sign_in;

use porter_core::{Account, AccountId, Audience, Claim, Credential, IssuedToken};
use porter_http::SharedSleep;
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, RevokeOutcome, SignInMode,
    SignInStart,
};
use porter_tailscale::LocalApi;

pub use sign_in::TailnetSignIn;

/// The Tailnet family's provider.
#[derive(Debug, Clone)]
pub struct TailnetProvider {
    spec: ProviderSpec,
    api: LocalApi,
    sleep: SharedSleep,
}

impl TailnetProvider {
    /// The provider serving the accounts of `spec`, asking Tailscale through `api`, and waiting
    /// between its questions through `sleep`.
    pub fn new(spec: ProviderSpec, api: LocalApi, sleep: SharedSleep) -> Self {
        Self { spec, api, sleep }
    }
}

/// An open Tailscale account: it holds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TailnetSession;

impl Provider for TailnetProvider {
    type Session = TailnetSession;
    type SignIn = TailnetSignIn;

    fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    async fn discover(
        &self,
        _account: &Account,
        _presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        // Nothing for an app to ask Accounts1 for: the service is `Tailnet1`.
        Ok(Vec::new())
    }

    async fn open(
        &self,
        _account: &AccountId,
        presented: Presented,
    ) -> Result<TailnetSession, ProviderError> {
        match presented {
            Presented::Anonymous => Ok(TailnetSession),
            Presented::Credential(_) => Err(ProviderError::Unreadable),
        }
    }

    fn sign_in(&self, start: SignInStart) -> Result<TailnetSignIn, ProviderError> {
        // Signing in again is the same conversation as adding, ending without a review: the
        // account is already there.
        let review = matches!(start.mode, SignInMode::Add);
        Ok(TailnetSignIn::new(
            self.api.clone(),
            self.sleep.clone(),
            review,
        ))
    }

    async fn revoke(
        &self,
        _account: &Account,
        _presented: &Presented,
    ) -> Result<RevokeOutcome, ProviderError> {
        // Tailscale stays signed in: only the person signs it out, in Tailscale.
        Ok(RevokeOutcome::Unsupported)
    }
}

impl ProviderSession for TailnetSession {
    async fn access_token(&self, _audience: &Audience) -> Result<IssuedToken, ProviderError> {
        Err(ProviderError::Forbidden)
    }

    fn renewed(&self) -> Option<Credential> {
        None
    }
}
