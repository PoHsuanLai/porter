//! The OAuth family: an issuer's accounts that sign in by PKCE through a loopback redirect in the
//! person's browser and are kept by a refresh token. One provider, one sign-in and one session
//! serve every issuer; what an issuer publishes is data (`issuers.toml`) and what is left is its
//! [`BrowserIssuer`] rules: `Google`, `Microsoft`, and [`Standard`] for an issuer that needs
//! none.
//!
//! Everything it touches comes in through [`OAuthEnv`] (the `Http` seam, the client registry, the
//! clock, randomness), so tests drive it against a fake issuer. With no client row the provider
//! is still served: signing in answers `NeedsClientId`, which the sheet words as "sign-in needs a
//! client id".

mod env;
mod flow;
mod identity;
mod issuer;
mod session;
mod standard;

pub use env::OAuthEnv;
pub use flow::{Browser, BrowserFlow, Cx, Grant};
pub use issuer::BrowserIssuer;
pub use session::OAuthSession;
pub use standard::Standard;

pub(crate) use flow::{exchange_fault, provider_fault};
pub(crate) use identity::{failure, get, userinfo_address};

use porter_capability::Claim;
use porter_core::{Account, AccountId, Credential, SecretText};
use porter_family_kit::Conversation;
use porter_http::Http;
use porter_http_reqwest::ReqwestHttp;
use porter_oauth::{ExchangeFault, endpoints_of, revoke};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, Readiness, RevokeOutcome,
    SignInStart,
};
use std::collections::HashMap;

/// Refresh tokens that `discover` rotated, by account: `(the token it was given, what replaced
/// it)`. `discover` cannot return a credential, so the next `open` of the account, which
/// presents the same old token, starts from the new one and reports it through `renewed`.
type Rotations = std::sync::Arc<std::sync::Mutex<HashMap<AccountId, (SecretText, Credential)>>>;

/// The OAuth family's provider for the issuer `R`, over the HTTP seam `H` (the platform's client
/// in a daemon).
pub struct OAuthProvider<R, H = ReqwestHttp> {
    rules: R,
    spec: ProviderSpec,
    env: OAuthEnv<H>,
    rotations: Rotations,
}

impl<R: Clone, H> Clone for OAuthProvider<R, H> {
    fn clone(&self) -> Self {
        Self {
            rules: self.rules.clone(),
            spec: self.spec.clone(),
            env: self.env.clone(),
            rotations: self.rotations.clone(),
        }
    }
}

impl<R: std::fmt::Debug, H> std::fmt::Debug for OAuthProvider<R, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthProvider")
            .field("rules", &self.rules)
            .field("spec", &self.spec.id)
            .field("env", &self.env)
            .finish()
    }
}

impl<R, H> OAuthProvider<R, H> {
    /// The provider serving the accounts of `spec` in `env`, under `rules`.
    pub fn with_env(rules: R, spec: ProviderSpec, env: OAuthEnv<H>) -> Self {
        Self {
            rules,
            spec,
            env,
            rotations: Rotations::default(),
        }
    }

    fn rotations(&self) -> std::sync::MutexGuard<'_, HashMap<AccountId, (SecretText, Credential)>> {
        self.rotations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl<R: BrowserIssuer, H: Http + 'static> Provider for OAuthProvider<R, H> {
    type Session = OAuthSession<R, H>;
    type SignIn = Conversation<BrowserFlow<R, H>>;

    fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    async fn discover(
        &self,
        account: &Account,
        presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        let session = self.open(&account.id, presented.clone()).await?;
        let found = self.rules.discover(&session).await?;
        if let (Some(renewed), Presented::Credential(Credential::OAuth { refresh, .. })) =
            (session.renewed(), presented)
        {
            self.rotations()
                .insert(account.id.clone(), (refresh.clone(), renewed));
        }
        Ok(found)
    }

    async fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> Result<OAuthSession<R, H>, ProviderError> {
        let rotated = match &presented {
            Presented::Credential(Credential::OAuth { refresh, .. }) => {
                let mut held = self.rotations();
                match held.get(account) {
                    Some((from, _)) if from == refresh => held.remove(account).map(|(_, to)| to),
                    _ => None,
                }
            }
            _ => None,
        };
        let session = OAuthSession::new(
            self.rules.clone(),
            account.clone(),
            self.spec.clone(),
            self.env.clone(),
            presented,
        )?;
        Ok(match rotated {
            Some(credential) => session.adopt(credential),
            None => session,
        })
    }

    fn sign_in(&self, start: SignInStart) -> Result<Self::SignIn, ProviderError> {
        Ok(Conversation::new(BrowserFlow::new(
            self.rules.clone(),
            self.spec.clone(),
            self.env.clone(),
            start,
        )))
    }

    /// Ready while a client of the issuer is registered for this build's channel (shipped, or set
    /// in Settings: the files are read now).
    fn readiness(&self) -> Readiness {
        match self
            .env
            .clients()
            .lookup(self.rules.issuer(), self.env.channel)
        {
            Some(_) => Readiness::Ready,
            None => Readiness::NeedsClient,
        }
    }

    /// Revokes the refresh token at the issuer's revoke endpoint, which ends the access tokens
    /// made from it too (a token the issuer no longer knows is revoked already), or sends the
    /// person to the page where the issuer lists the apps it gave access to. An issuer with
    /// neither is `Unsupported`.
    async fn revoke(
        &self,
        _account: &Account,
        presented: &Presented,
    ) -> Result<RevokeOutcome, ProviderError> {
        let data = self.rules.issuer().data();
        if let Some(page) = &data.manual_revoke {
            return Ok(RevokeOutcome::Manual(page.clone()));
        }
        let Presented::Credential(Credential::OAuth { refresh, .. }) = presented else {
            return Ok(RevokeOutcome::Unsupported);
        };
        if !data.has_revoke() {
            return Ok(RevokeOutcome::Unsupported);
        }
        let clients = self.env.clients().into_owned();
        let Some(client) = clients.lookup(self.rules.issuer(), self.env.channel) else {
            // No client is configured: nothing can be asked of the issuer, and the account's own
            // secrets are deleted regardless.
            return Ok(RevokeOutcome::Unsupported);
        };
        match revoke(&*self.env.http, &endpoints_of(client), refresh).await {
            Ok(()) | Err(ExchangeFault::Refused) => Ok(RevokeOutcome::Revoked),
            Err(ExchangeFault::Unreachable) => Err(ProviderError::Unreachable),
            // unreadable, and a fault a later porter-oauth adds
            Err(_) => Ok(RevokeOutcome::Unsupported),
        }
    }
}
