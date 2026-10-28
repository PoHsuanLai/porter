//! An open OAuth account: the refresh token, and the short-lived tokens minted from it, one per
//! scope asked (`None`: the whole grant, which is porter's own and never leaves accountd). The
//! issuer's rules say which scope an audience or a grant is refreshed with and in which form (a
//! bearer for the APIs, an XOAUTH2 string for IMAP and SMTP), and whether the account may have it
//! at all.
//!
//! A refresh is the one place a credential changes: a rotated refresh token is kept, and so is a
//! change in what the grant holds, and the credential to store again is reported once through
//! `renewed`. A refresh the issuer refuses with `invalid_scope` (or Microsoft's `AADSTS65001`) is
//! `NeedsConsent`, never a sign-in again.

use super::env::OAuthEnv;
use super::issuer::BrowserIssuer;
use porter_capability::{CapabilityKind, StorageScope};
use porter_core::{AccountId, Audience, Credential, IssuedToken, TokenKind};
use porter_http::Http;
use porter_ids::UnixSeconds;
use porter_oauth::{ExchangeFault, endpoints_of, is_due, refresh_scoped_detailed};
use porter_provider::{Presented, ProviderError, ProviderSession, ProviderSpec};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

/// What every OAuth session keeps, and the issuer's own beside it.
#[derive(Debug)]
pub(crate) struct Core<S> {
    pub credential: Credential,
    /// The token last minted for each scope asked ("" for the whole grant).
    pub minted: HashMap<String, IssuedToken>,
    /// The credential to store again, once, after it changed.
    pub renewed: Option<Credential>,
    /// The account's address, read once: XOAUTH2 names the user.
    pub address: Option<String>,
    /// The issuer's.
    pub extra: S,
}

/// An open OAuth account of the issuer `R`.
pub struct OAuthSession<R: BrowserIssuer, H = porter_http_reqwest::ReqwestHttp> {
    rules: R,
    account: AccountId,
    spec: Box<ProviderSpec>,
    env: Box<OAuthEnv<H>>,
    state: Box<Mutex<Core<R::State>>>,
}

impl<R: BrowserIssuer, H> std::fmt::Debug for OAuthSession<R, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthSession")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl<R: BrowserIssuer, H> OAuthSession<R, H> {
    pub(crate) fn new(
        rules: R,
        account: AccountId,
        spec: ProviderSpec,
        env: OAuthEnv<H>,
        presented: Presented,
    ) -> Result<Self, ProviderError> {
        let Presented::Credential(credential @ Credential::OAuth { .. }) = presented else {
            return Err(ProviderError::Unauthorized);
        };
        let extra = rules.open_state(&credential);
        Ok(Self {
            rules,
            account,
            spec: Box::new(spec),
            env: Box::new(env),
            state: Box::new(Mutex::new(Core {
                credential,
                minted: HashMap::new(),
                renewed: None,
                address: None,
                extra,
            })),
        })
    }

    /// The same session continuing from `credential`, a rotation of the one it was opened with:
    /// it mints from the new token, and reports it as renewed.
    pub(crate) fn adopt(self, credential: Credential) -> Self {
        {
            let mut state = self.state();
            state.extra = self.rules.open_state(&credential);
            state.credential = credential.clone();
            state.renewed = Some(credential);
        }
        self
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, Core<R::State>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The provider file.
    pub(crate) fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    /// The surroundings.
    pub(crate) fn env(&self) -> &OAuthEnv<H> {
        &self.env
    }

    /// Whether the client is the person's own, which is what lets restricted scopes be asked.
    pub(crate) fn byo(&self) -> bool {
        self.env
            .clients()
            .traits(self.rules.issuer(), self.env.channel)
            .byo
    }

    fn fresh(&self, key: &str, now: UnixSeconds) -> Option<IssuedToken> {
        self.state()
            .minted
            .get(key)
            .filter(|t| !is_due(t.expires, now))
            .cloned()
    }
}

impl<R: BrowserIssuer, H: Http + 'static> OAuthSession<R, H> {
    /// The address the account signs in with (once).
    pub(crate) async fn address(&self) -> Result<String, ProviderError> {
        if let Some(address) = self.state().address.clone() {
            return Ok(address);
        }
        let address = self.rules.identify(self).await?;
        self.state().address = Some(address.clone());
        Ok(address)
    }

    /// A raw bearer token for `scope` (space separated), or for the whole grant when `None`,
    /// reused while fresh, renewed from the refresh token.
    pub(crate) async fn mint(&self, scope: Option<&str>) -> Result<IssuedToken, ProviderError> {
        let key = scope.unwrap_or_default();
        let now = (self.env.clock)();
        if let Some(token) = self.fresh(key, now) {
            return Ok(token);
        }
        let Credential::OAuth { refresh, .. } = self.state().credential.clone() else {
            return Err(ProviderError::Unauthorized);
        };
        // No client id configured is a fault of this install, not of the account: offline, so
        // the person is not asked to sign in again for it.
        let clients = self.env.clients().into_owned();
        let client = clients
            .lookup(self.rules.issuer(), self.env.channel)
            .ok_or(ProviderError::Unreachable)?;
        let tokens = refresh_scoped_detailed(
            &*self.env.http,
            &endpoints_of(client),
            client,
            &refresh,
            scope,
        )
        .await
        .map_err(|failure| match failure.fault {
            // `invalid_scope`, `AADSTS65001`: the grant is good, the scope was never given.
            _ if failure.wants_consent() => ProviderError::NeedsConsent,
            ExchangeFault::Refused => {
                let testing = clients
                    .traits(self.rules.issuer(), self.env.channel)
                    .testing;
                self.rules.refused(&mut self.state().extra, testing, now);
                ProviderError::Unauthorized
            }
            ExchangeFault::Unreachable => ProviderError::Unreachable,
            // unreadable, and a fault a later porter-oauth adds
            _ => ProviderError::Unreadable,
        })?;
        let issued = IssuedToken::new(
            TokenKind::Bearer,
            tokens.access_token.clone(),
            tokens.expires_at(now),
        );
        let mut state = self.state();
        let current = match &state.credential {
            Credential::OAuth { granted, .. } => granted.clone(),
            _ => porter_core::GrantedScopes::unknown(),
        };
        let record = self
            .rules
            .answered(&mut state.extra, &current, scope, &tokens);
        // A refresh token that came back different is kept, and so is a change in what the
        // grant holds.
        let changed = tokens.refresh_token.filter(|r| *r != refresh);
        if changed.is_some() || record != current {
            let credential = Credential::OAuth {
                access: tokens.access_token,
                refresh: changed.unwrap_or(refresh),
                expires_at: issued.expires,
                granted: record,
            };
            state.credential = credential.clone();
            state.renewed = Some(credential);
        }
        state.minted.insert(key.to_owned(), issued.clone());
        Ok(issued)
    }

    /// `token` in the form `form` says: an XOAUTH2 string names the account.
    async fn shaped(
        &self,
        token: IssuedToken,
        form: TokenKind,
    ) -> Result<IssuedToken, ProviderError> {
        match form {
            TokenKind::Xoauth2 => {
                let user = self.address().await?;
                Ok(IssuedToken::new(
                    form,
                    porter_core::token::xoauth2(&user, token.value.expose()),
                    token.expires,
                ))
            }
            _ => Ok(token),
        }
    }
}

impl<R: BrowserIssuer, H: Http + 'static> ProviderSession for OAuthSession<R, H> {
    /// A token for porter's own use, for every scope granted. The host gives apps and relays
    /// only [`Self::access_token_for`]'s.
    async fn access_token(&self, audience: &Audience) -> Result<IssuedToken, ProviderError> {
        let (scope, form) = self.rules.porter_token(self, audience)?;
        let token = self.mint(scope.as_deref()).await?;
        self.shaped(token, form).await
    }

    /// A token refreshed with the scopes of `kind` alone, where the issuer scopes its tokens.
    async fn access_token_for(
        &self,
        audience: &Audience,
        kind: CapabilityKind,
        reach: StorageScope,
    ) -> Result<IssuedToken, ProviderError> {
        let (scope, form) = self.rules.grant_token(self, audience, kind, reach)?;
        let token = self.mint(scope.as_deref()).await?;
        self.shaped(token, form).await
    }

    fn renewed(&self) -> Option<Credential> {
        self.state().renewed.take()
    }
}
