//! An open Microsoft account: the refresh token, and the short-lived tokens minted from it, one
//! per scope (Exchange for IMAP and SMTP; for Graph, the one permission of a grant's kind).

use super::env::MicrosoftEnv;
use super::graph::{Found, probe, whoami};
use super::graph_origin;
use super::scopes::{audience_scope, grant_scope};
use porter_core::{
    AccountId, Audience, CapabilityKind, Credential, IssuedToken, SecretText, TokenKind,
    UnixSeconds,
};
use porter_http::Http;
use porter_oauth::{ExchangeFault, Renewal, endpoints_of, refresh_scoped, renewal};
use porter_provider::{Presented, ProviderError, ProviderSession, ProviderSpec};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

/// An open Microsoft account.
pub struct MicrosoftSession<H = porter_http::HyperHttp> {
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
        let Credential::OAuth { refresh, .. } = self.state().credential.clone() else {
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
        let tokens = refresh_scoped(
            &*self.env.http,
            &endpoints_of(&client),
            &client,
            &refresh,
            Some(&scope),
        )
        .await
        .map_err(|fault| match fault {
            ExchangeFault::Refused => ProviderError::Unauthorized,
            ExchangeFault::Unreachable => ProviderError::Unreachable,
            ExchangeFault::Unreadable => ProviderError::Unreadable,
        })?;
        let issued = IssuedToken::new(kind, tokens.access_token.clone(), tokens.expires_at(now));
        let mut state = self.state();
        if let Some(rotated) = tokens.refresh_token.filter(|r| *r != refresh) {
            let credential = Credential::OAuth {
                access: tokens.access_token,
                refresh: rotated,
                expires_at: issued.expires,
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
                    SecretText::new(format!(
                        "user={user}\u{1}auth=Bearer {}\u{1}\u{1}",
                        token.value.expose()
                    )),
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
    /// token is for `Calendars.ReadWrite`, so it cannot read the account's files or mail.
    async fn access_token_for(
        &self,
        audience: &Audience,
        kind: CapabilityKind,
    ) -> Result<IssuedToken, ProviderError> {
        let graph = graph_origin(&self.spec);
        let (scope, form) =
            grant_scope(audience, kind, graph.as_str()).ok_or(ProviderError::Forbidden)?;
        self.issue(&scope, form).await
    }

    fn renewed(&self) -> Option<Credential> {
        self.state().renewed.take()
    }
}
