//! The browser half of an OAuth sign-in, written once: PKCE from random bytes, a loopback
//! listener for the redirect, the authorize URL, the wait for the person, and the exchange of the
//! code. What an issuer does with the tokens (read the account, probe its services) is its
//! [`BrowserIssuer`].
//!
//! The host feeds `Start`, then `Poll` until the browser has answered, then `Confirm`. `Cancel`
//! ends it at any step and frees the listener. Signing in again (`SignInMode::Reauthenticate`)
//! has no review: it is `Done` after the issuer has read the account. Asking for one more access
//! (`SignInMode::Extend`) asks the issuer for that access alone and reviews it before it is
//! `Done`. With no client row for this build the first step is `Failed(NeedsClientId)`: nothing
//! is opened and nothing is dialled.

use super::env::OAuthEnv;
use super::issuer::BrowserIssuer;
use crate::conversation::{Flow, Move};
use porter_http::Http;
use porter_ids::WebUrl;
use porter_oauth::{
    AuthCode, AuthorizeAsk, ClientTraits, ExchangeFault, LoopbackFault, LoopbackServer, OAuthState,
    Pkce, TokenResponse, authorize_url_for, endpoints_of, exchange_code_scoped,
};
use porter_provider::{
    ClientEntry, IssuerEndpoints, ProviderError, ProviderSpec, SignInMode, SignInStart, SignInStep,
    Signed,
};
use porter_sheet::SignInFault;
use tokio::task::JoinHandle;

/// What a sign-in asked for: the client, where it signs in, and the scopes.
#[derive(Debug)]
pub struct Grant {
    /// The client row of this build.
    pub client: ClientEntry,
    /// Where the issuer's pages and token endpoint are.
    pub endpoints: IssuerEndpoints,
    /// What the client row says besides its id.
    pub traits: ClientTraits,
    /// The scopes asked.
    pub scopes: Vec<String>,
}

/// What an issuer's rules read a sign-in through.
pub struct Cx<'a, H> {
    /// The provider file.
    pub spec: &'a ProviderSpec,
    /// The surroundings.
    pub env: &'a OAuthEnv<H>,
}

impl<H> std::fmt::Debug for Cx<'_, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cx")
            .field("spec", &self.spec.id)
            .field("env", &self.env)
            .finish()
    }
}

/// A task that is aborted when its owner goes away.
#[derive(Debug)]
struct Task<T>(JoinHandle<T>);

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The browser is on its way back to the loopback listener.
#[derive(Debug)]
pub struct Browser {
    grant: Grant,
    pkce: Pkce,
    redirect: String,
    wait: Task<Result<AuthCode, LoopbackFault>>,
}

/// The browser sign-in of the issuer `R`, over the HTTP seam `H`.
pub struct BrowserFlow<R, H> {
    rules: R,
    spec: Box<ProviderSpec>,
    env: Box<OAuthEnv<H>>,
    start: SignInStart,
}

impl<R: std::fmt::Debug, H> std::fmt::Debug for BrowserFlow<R, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserFlow")
            .field("rules", &self.rules)
            .field("start", &self.start)
            .finish_non_exhaustive()
    }
}

impl<R, H> BrowserFlow<R, H> {
    pub(super) fn new(rules: R, spec: ProviderSpec, env: OAuthEnv<H>, start: SignInStart) -> Self {
        Self {
            rules,
            spec: Box::new(spec),
            env: Box::new(env),
            start,
        }
    }
}

type Step = Result<Move<Browser>, SignInFault>;

impl<R: BrowserIssuer, H: Http + 'static> BrowserFlow<R, H> {
    fn cx(&self) -> Cx<'_, H> {
        Cx {
            spec: &self.spec,
            env: &self.env,
        }
    }

    async fn begin(&mut self) -> Step {
        let clients = self.env.clients().into_owned();
        let issuer = self.rules.issuer();
        let client = clients
            .lookup(issuer, self.env.channel)
            .cloned()
            .ok_or(SignInFault::NeedsClientId)?;
        let traits = clients.traits(issuer, self.env.channel);
        let scopes = self.rules.scopes(&self.spec, &self.start.mode, traits.byo);
        let (verifier, state) = (self.env.random)().ok_or(SignInFault::Unreadable)?;
        let pkce = Pkce::from_random(verifier, state);
        let server = LoopbackServer::bind()
            .await
            .map_err(|_| SignInFault::Unreachable)?;
        let redirect = server.redirect_uri();
        let endpoints = endpoints_of(&client);
        let target = authorize_url_for(
            &endpoints,
            &client,
            &pkce,
            &redirect,
            &scopes,
            &AuthorizeAsk::of(&self.start),
        );
        let url = WebUrl::parse(&target).map_err(|_| SignInFault::Unreadable)?;
        let expected: OAuthState = pkce.state.clone();
        let wait = Task(tokio::spawn(async move { server.wait(&expected).await }));
        let grant = Grant {
            client,
            endpoints,
            traits,
            scopes,
        };
        let browser = Browser {
            grant,
            pkce,
            redirect,
            wait,
        };
        Ok(Move::Next(SignInStep::OpenBrowser { url }, browser))
    }

    async fn await_browser(&mut self, mut browser: Browser) -> Step {
        let arrived = tokio::time::timeout(self.env.poll_slice, &mut browser.wait.0).await;
        let code = match arrived {
            Err(_elapsed) => return Ok(Move::Next(SignInStep::Waiting, browser)),
            Ok(Err(_panicked)) => return Err(SignInFault::Unreadable),
            Ok(Ok(outcome)) => outcome.map_err(|fault| loopback_fault(&self.rules, fault))?,
        };
        let Browser {
            grant,
            pkce,
            redirect,
            ..
        } = browser;
        let tokens = exchange_code_scoped(
            &*self.env.http,
            &grant.endpoints,
            &grant.client,
            &pkce,
            &code,
            &redirect,
            self.rules.redeem_scope(&grant.scopes).as_deref(),
        )
        .await
        .map_err(exchange_fault)?;
        self.conclude(grant, tokens).await
    }

    /// The tokens are in: read the account, and offer the review.
    async fn conclude(&mut self, grant: Grant, tokens: TokenResponse) -> Step {
        let cx = self.cx();
        if let SignInMode::Extend {
            endpoints, kinds, ..
        } = &self.start.mode
        {
            let signed = self
                .rules
                .extended(&cx, &grant, tokens, endpoints, kinds)
                .await?;
            return Ok(Move::Review(signed));
        }
        let signed: Signed = self.rules.signed(&cx, grant, tokens).await?;
        // Signing in again does not ask what to use again: the account keeps its services.
        Ok(match self.start.mode {
            SignInMode::Reauthenticate { .. } => Move::Done(signed),
            _ => Move::Review(signed),
        })
    }
}

impl<R: BrowserIssuer, H: Http + 'static> Flow for BrowserFlow<R, H> {
    type Mid = Browser;

    async fn start(&mut self) -> Move<Browser> {
        self.begin().await.unwrap_or_else(Move::Fail)
    }

    async fn resume(
        &mut self,
        browser: Browser,
        _input: porter_sheet::SignInInput,
    ) -> Move<Browser> {
        self.await_browser(browser).await.unwrap_or_else(Move::Fail)
    }
}

/// What a token endpoint's refusal means to the person.
pub(crate) fn exchange_fault(fault: ExchangeFault) -> SignInFault {
    match fault {
        ExchangeFault::Refused => SignInFault::Refused,
        ExchangeFault::Unreachable => SignInFault::Unreachable,
        // unreadable, and a fault a later porter-oauth adds
        _ => SignInFault::Unreadable,
    }
}

/// What an API's refusal means to the person.
pub(crate) fn provider_fault(error: ProviderError) -> SignInFault {
    match error {
        ProviderError::Unauthorized => SignInFault::Refused,
        ProviderError::Forbidden => SignInFault::Forbidden,
        ProviderError::Unreachable => SignInFault::Unreachable,
        ProviderError::Unreadable => SignInFault::Unreadable,
        // A fault this family does not know yet is read as an answer it cannot read.
        _ => SignInFault::Unreadable,
    }
}

/// What the issuer's redirect (or the lack of one) says of the sign-in, in the words of the
/// issuer's rules ([`BrowserIssuer::refusal`]).
pub(crate) fn loopback_fault<R: BrowserIssuer>(rules: &R, fault: LoopbackFault) -> SignInFault {
    match fault {
        LoopbackFault::TimedOut => SignInFault::TimedOut,
        LoopbackFault::Refused(error) => rules.refusal(&error),
        LoopbackFault::WrongState => SignInFault::Refused,
        // oversized, no code, malformed, and a fault a later porter-oauth adds
        _ => SignInFault::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::Standard;
    use porter_provider::Issuer;

    #[test]
    fn a_redirect_fault_maps_to_what_the_person_sees() {
        const CASES: &[(LoopbackFault, SignInFault)] = &[
            (LoopbackFault::TimedOut, SignInFault::TimedOut),
            (LoopbackFault::WrongState, SignInFault::Refused),
            (LoopbackFault::NoCode, SignInFault::Unreadable),
            (LoopbackFault::Malformed, SignInFault::Unreadable),
            (LoopbackFault::Oversized, SignInFault::Unreadable),
        ];
        let rules = Standard::of(Issuer::Yahoo);
        for (fault, want) in CASES {
            assert_eq!(loopback_fault(&rules, fault.clone()), *want, "{fault:?}");
        }
    }

    #[test]
    fn a_person_who_said_no_cancelled_and_the_rest_were_refused() {
        let rules = Standard::of(Issuer::Yahoo);
        assert_eq!(
            loopback_fault(&rules, LoopbackFault::Refused("access_denied".into())),
            SignInFault::Cancelled
        );
        assert_eq!(
            loopback_fault(&rules, LoopbackFault::Refused("server_error".into())),
            SignInFault::Refused
        );
    }
}
