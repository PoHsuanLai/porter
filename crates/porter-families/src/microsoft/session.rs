//! An open Microsoft account: the refresh token, and the short-lived tokens minted from it, one
//! per scope (Exchange for IMAP and SMTP; for Graph, the one permission of a grant's kind).
//!
//! A grant whose scope the account was not given is refused as `NeedsConsent` before Microsoft
//! is asked: the credential keeps the scopes its sign-in asked (Microsoft's consent is all of
//! them or none, and a refresh's answer names only the one resource it is for). A credential
//! from before porter kept them is taken to hold what sign-in asks and nothing asked on use.
//! Microsoft's own `AADSTS65001` on a refresh is the same refusal, never a sign-in again.

use super::env::MicrosoftEnv;
use super::graph::{Found, probe, whoami};
use super::graph_origin;
use super::scopes::{audience_scope, grant_scope};
use porter_core::AccountId;
use porter_core::capability::{Ask, StorageScope};
use porter_core::{Audience, CapabilityKind, Credential, IssuedToken, TokenKind};
use porter_http::Http;
use porter_ids::UnixSeconds;
use porter_oauth::{ExchangeFault, Renewal, endpoints_of, refresh_scoped_detailed, renewal};
use porter_provider::{Presented, ProviderError, ProviderSession, ProviderSpec};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

/// An open Microsoft account.
pub struct MicrosoftSession<H = porter_http_reqwest::ReqwestHttp> {
    account: AccountId,
    spec: Box<ProviderSpec>,
    env: Box<MicrosoftEnv<H>>,
    state: Box<Mutex<State>>,
}

#[derive(Debug)]
struct State {
    credential: Credential,
    /// The token last minted for each resource scope.
    minted: HashMap<String, IssuedToken>,
    /// The credential to store again, once, after the refresh token rotated.
    renewed: Option<Credential>,
    /// The mailbox address, read once from Graph: XOAUTH2 names the user.
    address: Option<String>,
}

impl<H> std::fmt::Debug for MicrosoftSession<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MicrosoftSession")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl<H: Http> MicrosoftSession<H> {
    pub(super) fn new(
        account: AccountId,
        spec: ProviderSpec,
        env: MicrosoftEnv<H>,
        presented: Presented,
    ) -> Result<Self, ProviderError> {
        let Presented::Credential(credential @ Credential::OAuth { .. }) = presented else {
            return Err(ProviderError::Unauthorized);
        };
        Ok(Self {
            account,
            spec: Box::new(spec),
            env: Box::new(env),
            state: Box::new(Mutex::new(State {
                credential,
                minted: HashMap::new(),
                renewed: None,
                address: None,
            })),
        })
    }

    /// The same session continuing from `credential`, a rotation of the one it was opened with:
    /// it mints from the new token, and reports it as renewed.
    pub(super) fn adopt(self, credential: Credential) -> Self {
        {
            let mut state = self.state();
            state.credential = credential.clone();
            state.renewed = Some(credential);
        }
        self
    }

    /// What the account's Graph services answer.
    pub(super) async fn probe(&self) -> Result<Found, ProviderError> {
        let base = graph_origin(&self.spec);
        let token = self
            .access_token(&Audience(base.as_str().to_owned()))
            .await?;
        probe(&*self.env.http, &self.spec, &base, token.value.expose()).await
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether a grant asked as `ask` may be refreshed with `scope`. Microsoft's consent is all
    /// or nothing, so what sign-in asks was given; an on-use scope only once the credential says
    /// so.
    fn given(&self, scope: &str, ask: Ask) -> bool {
        match (&self.state().credential, ask) {
            (_, Ask::SignIn) => true,
            (Credential::OAuth { granted, .. }, Ask::OnUse) => granted.holds(scope) == Some(true),
            _ => false,
        }
    }

    fn fresh(&self, scope: &str, now: UnixSeconds) -> Option<IssuedToken> {
        self.state()
            .minted
            .get(scope)
            .filter(|t| renewal(t.expires, now) == Renewal::Fresh)
            .cloned()
    }
}

impl<H: Http> MicrosoftSession<H> {
    /// The address the account signs in with, from Graph's `/me` (once).
    async fn address(&self) -> Result<String, ProviderError> {
        if let Some(address) = self.state().address.clone() {
            return Ok(address);
        }
        let base = graph_origin(&self.spec);
        // The profile scope alone: the address is all this token is for.
        let graph = self
            .mint(super::scopes::USER_READ, TokenKind::Bearer)
            .await?;
        let address = whoami(&*self.env.http, &base, graph.value.expose()).await?;
        self.state().address = Some(address.clone());
        Ok(address)
    }

    /// A raw access token for `scope`, reused while fresh, renewed from the refresh token.
    async fn mint(&self, scope: &str, kind: TokenKind) -> Result<IssuedToken, ProviderError> {
        let scope = scope.to_owned();
        let now = (self.env.clock)();
        if let Some(token) = self.fresh(&scope, now) {
            return Ok(token);
        }
        let Credential::OAuth {
            refresh, granted, ..
        } = self.state().credential.clone()
        else {
            return Err(ProviderError::Unauthorized);
        };
        // No client id configured is a fault of this install, not of the account: offline, so
        // the person is not asked to sign in again for it.
        let client = self
            .env
            .clients()
            .lookup(porter_provider::Issuer::Microsoft, self.env.channel)
            .cloned()
            .ok_or(ProviderError::Unreachable)?;
        let tokens = refresh_scoped_detailed(
            &*self.env.http,
            &endpoints_of(&client),
            &client,
            &refresh,
            Some(&scope),
        )
        .await
        .map_err(|failure| match failure.fault {
            // `AADSTS65001` comes as `invalid_grant`: the grant is good, the scope was never
            // consented.
            _ if failure.wants_consent() => ProviderError::NeedsConsent,
            ExchangeFault::Refused => ProviderError::Unauthorized,
            ExchangeFault::Unreachable => ProviderError::Unreachable,
            // unreadable, and a fault a later porter-oauth adds
            _ => ProviderError::Unreadable,
        })?;
        let issued = IssuedToken::new(kind, tokens.access_token.clone(), tokens.expires_at(now));
        let mut state = self.state();
        if let Some(rotated) = tokens.refresh_token.filter(|r| *r != refresh) {
            let credential = Credential::OAuth {
                access: tokens.access_token,
                refresh: rotated,
                expires_at: issued.expires,
                granted,
            };
            state.credential = credential.clone();
            state.renewed = Some(credential);
        }
        state.minted.insert(scope, issued.clone());
        Ok(issued)
    }
}

impl<H: Http> MicrosoftSession<H> {
    /// The token minted for `scope`, in the form `kind` says: an XOAUTH2 string names the
    /// mailbox.
    async fn issue(&self, scope: &str, kind: TokenKind) -> Result<IssuedToken, ProviderError> {
        let token = self.mint(scope, kind).await?;
        match kind {
            TokenKind::Xoauth2 => {
                let user = self.address().await?;
                Ok(IssuedToken::new(
                    token.kind,
                    porter_core::token::xoauth2(&user, token.value.expose()),
                    token.expires,
                ))
            }
            _ => Ok(token),
        }
    }
}

impl<H: Http> ProviderSession for MicrosoftSession<H> {
    /// A token for porter's own use: Graph's is for every permission consented (`.default`).
    /// The host gives apps and relays only [`Self::access_token_for`]'s.
    async fn access_token(&self, audience: &Audience) -> Result<IssuedToken, ProviderError> {
        let graph = graph_origin(&self.spec);
        let (scope, kind) =
            audience_scope(audience, graph.as_str()).ok_or(ProviderError::Forbidden)?;
        self.issue(&scope, kind).await
    }

    /// A token carrying the scope of `kind` alone (`grant_scope`): a calendar grant's Graph
    /// token is for `Calendars.ReadWrite`, so it cannot read the account's files or mail. The
    /// row is the one of the grant's `reach`: a Storage grant of the app folder is served by the
    /// sign-in row (`Files.ReadWrite.AppFolder`), one of the whole store by the on-use row
    /// (`Files.ReadWrite`), which is `NeedsConsent` until the account was given that access.
    async fn access_token_for(
        &self,
        audience: &Audience,
        kind: CapabilityKind,
        reach: StorageScope,
    ) -> Result<IssuedToken, ProviderError> {
        let graph = graph_origin(&self.spec);
        let ask = self
            .spec
            .ask_for(kind, reach)
            .ok_or(ProviderError::Forbidden)?;
        let (scope, form) =
            grant_scope(audience, kind, ask, graph.as_str()).ok_or(ProviderError::Forbidden)?;
        if !self.given(&scope, ask) {
            return Err(ProviderError::NeedsConsent);
        }
        self.issue(&scope, form).await
    }

    fn renewed(&self) -> Option<Credential> {
        self.state().renewed.take()
    }
}
