//! An open Google account: the refresh token, and the short-lived tokens minted from it.
//! Porter's own token (the add-time probe, the account's address) carries every scope granted
//! at sign-in and never leaves accountd. A token for a grant is refreshed with the scopes of the
//! grant's kind alone (`grant_scopes`), so a calendar grant's token cannot read the account's
//! Drive. The audience says which form it takes (a bearer for the APIs, an XOAUTH2 string for
//! IMAP and SMTP) and whether the account may have it at all.
//!
//! Narrowing on a refresh follows Google's documented rule, unverified against Google itself
//! (FINDINGS): the scopes named must be a subset of the original grant. So a refresh never names
//! a scope the last token answer said was not granted (granular consent).

use super::env::GoogleEnv;
use super::probe::{self, whoami};
use super::reauth::ReauthReason;
use super::scopes::{Granted, grant_scopes};
use porter_core::{
    AccountId, Audience, CapabilityKind, Claim, Credential, Family, IssuedToken, SecretText,
    TokenKind, UnixSeconds,
};
use porter_http::Http;
use porter_oauth::{ExchangeFault, MailRights, Renewal, endpoints_of, refresh_scoped, renewal};
use porter_provider::{Issuer, Presented, ProviderError, ProviderSession, ProviderSpec};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

/// An open Google account.
pub struct GoogleSession<H = porter_http::HyperHttp> {
    account: AccountId,
    spec: Box<ProviderSpec>,
    env: Box<GoogleEnv<H>>,
    state: Box<Mutex<State>>,
}

#[derive(Debug)]
struct State {
    credential: Credential,
    /// The token last minted for each scope asked ("" for the whole grant, porter's own).
    minted: HashMap<String, IssuedToken>,
    /// The scopes the last token answer said were granted.
    granted: Granted,
    /// The credential to store again, once, after the refresh token changed.
    renewed: Option<Credential>,
    /// The account's address, read once from userinfo: XOAUTH2 names the user.
    address: Option<String>,
    /// When the person signed in, if the host knows.
    signed_in: Option<UnixSeconds>,
    /// Why the last refresh was refused.
    reauth: Option<ReauthReason>,
}

impl<H> std::fmt::Debug for GoogleSession<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleSession")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl<H: Http> GoogleSession<H> {
    pub(super) fn new(
        account: AccountId,
        spec: ProviderSpec,
        env: GoogleEnv<H>,
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
                granted: Granted::of(Vec::new()),
                renewed: None,
                address: None,
                signed_in: None,
                reauth: None,
            })),
        })
    }

    /// The same session knowing the person signed in at `at`, so a refusal can be told from
    /// Google's seven days (`ReauthReason::of_refusal`). accountd does not keep that date yet
    /// (FINDINGS), so `open` leaves it unknown.
    pub fn signed_in_at(self, at: UnixSeconds) -> Self {
        self.state().signed_in = Some(at);
        self
    }

    /// Why the last refresh was refused, in words for the sheet, once one has been: the account
    /// is then `NeedsReauth` and this is what to tell the person.
    pub fn reauth_reason(&self) -> Option<ReauthReason> {
        self.state().reauth
    }

    /// What the account's services answer.
    pub(super) async fn probe(&self) -> Result<Vec<Claim>, ProviderError> {
        let token = self.mint().await?;
        let address = self.address().await?;
        let granted = self.state().granted.clone();
        probe::probe(
            &*self.env.http,
            &self.spec,
            &address,
            token.value.expose(),
            (&granted, self.mail_rights()),
        )
        .await
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn mail_rights(&self) -> MailRights {
        self.env
            .registry
            .traits(Issuer::Google, self.env.channel)
            .mail
    }

    fn fresh(&self, key: &str, now: UnixSeconds) -> Option<IssuedToken> {
        self.state()
            .minted
            .get(key)
            .filter(|t| renewal(t.expires, now) == Renewal::Fresh)
            .cloned()
    }

    /// The address the account signs in with, from userinfo (once).
    async fn address(&self) -> Result<String, ProviderError> {
        if let Some(address) = self.state().address.clone() {
            return Ok(address);
        }
        let token = self.mint().await?;
        let address = whoami(&*self.env.http, &self.env.userinfo, token.value.expose()).await?;
        self.state().address = Some(address.clone());
        Ok(address)
    }

    /// Porter's own access token, for every scope granted: reused while fresh, renewed from the
    /// refresh token. It never leaves accountd.
    async fn mint(&self) -> Result<IssuedToken, ProviderError> {
        self.mint_scoped(None).await
    }

    /// An access token for `scope` (space separated), or for the whole grant when `None`,
    /// reused while fresh, renewed from the refresh token.
    async fn mint_scoped(&self, scope: Option<&str>) -> Result<IssuedToken, ProviderError> {
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
        let client = self
            .env
            .registry
            .lookup(Issuer::Google, self.env.channel)
            .ok_or(ProviderError::Unreachable)?;
        let tokens = refresh_scoped(
            &*self.env.http,
            &endpoints_of(client),
            client,
            &refresh,
            scope,
        )
        .await
        .map_err(|fault| match fault {
            ExchangeFault::Refused => {
                let review = self
                    .env
                    .registry
                    .traits(Issuer::Google, self.env.channel)
                    .review;
                let mut state = self.state();
                state.reauth = Some(ReauthReason::of_refusal(review, state.signed_in, now));
                ProviderError::Unauthorized
            }
            ExchangeFault::Unreachable => ProviderError::Unreachable,
            ExchangeFault::Unreadable => ProviderError::Unreadable,
        })?;
        let issued = IssuedToken {
            kind: TokenKind::Bearer,
            expires: tokens.expires_at(now),
            value: tokens.access_token.clone(),
        };
        let mut state = self.state();
        state.reauth = None;
        // Only the whole grant's answer says what was granted; a narrowed one names its own.
        if scope.is_none() && !tokens.granted_scopes().is_empty() {
            state.granted = Granted::of(tokens.granted_scopes());
        }
        // Google does not rotate refresh tokens; one that came back different is kept.
        if let Some(changed) = tokens.refresh_token.filter(|r| *r != refresh) {
            let credential = Credential::OAuth {
                access: tokens.access_token,
                refresh: changed,
                expires_at: issued.expires,
            };
            state.credential = credential.clone();
            state.renewed = Some(credential);
        }
        state.minted.insert(key.to_owned(), issued.clone());
        Ok(issued)
    }

    /// The family a grant for `kind` reaches at `audience` and the form of its token, or
    /// `Forbidden` when no row of that kind names the audience (mail's IMAP and SMTP only for a
    /// person's own client).
    fn grant_target(
        &self,
        audience: &Audience,
        kind: CapabilityKind,
    ) -> Result<(Family, TokenKind), ProviderError> {
        let wanted = audience.0.trim_end_matches('/');
        let mail = |family| match (kind, self.mail_rights()) {
            (CapabilityKind::Mail, MailRights::Byo) => Ok((family, TokenKind::Xoauth2)),
            _ => Err(ProviderError::Forbidden),
        };
        match wanted {
            "imap" => return mail(Family::Imap),
            "smtp" => return mail(Family::Smtp),
            _ => {}
        }
        self.spec
            .capabilities
            .iter()
            .filter(|row| row.capability.kind() == kind)
            .find(|row| {
                row.family.slug() == wanted
                    || row
                        .endpoint
                        .as_ref()
                        .is_some_and(|e| e.0.trim_end_matches('/') == wanted)
            })
            .map(|row| (row.family, TokenKind::Bearer))
            .ok_or(ProviderError::Forbidden)
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
                Ok(IssuedToken {
                    kind: form,
                    value: SecretText::new(format!(
                        "user={user}\u{1}auth=Bearer {}\u{1}\u{1}",
                        token.value.expose()
                    )),
                    ..token
                })
            }
            _ => Ok(token),
        }
    }

    /// The form of token `audience` takes, or `Forbidden` for one that is not the account's.
    fn token_kind(&self, audience: &Audience) -> Result<TokenKind, ProviderError> {
        let wanted = audience.0.trim_end_matches('/');
        if wanted == "imap" || wanted == "smtp" {
            return match self.mail_rights() {
                MailRights::Byo => Ok(TokenKind::Xoauth2),
                MailRights::Withheld => Err(ProviderError::Forbidden),
            };
        }
        let named = self.spec.capabilities.iter().any(|row| {
            row.family.slug() == wanted
                || row
                    .endpoint
                    .as_ref()
                    .is_some_and(|e| e.0.trim_end_matches('/') == wanted)
        });
        match named {
            true => Ok(TokenKind::Bearer),
            false => Err(ProviderError::Forbidden),
        }
    }
}

impl<H: Http> ProviderSession for GoogleSession<H> {
    /// A token for porter's own use, for every scope granted. The host gives apps and relays
    /// only [`Self::access_token_for`]'s.
    async fn access_token(&self, audience: &Audience) -> Result<IssuedToken, ProviderError> {
        let form = self.token_kind(audience)?;
        let token = self.mint().await?;
        self.shaped(token, form).await
    }

    /// A token refreshed with the scopes of `kind` alone (`grant_scopes`). A scope the last
    /// token answer said was not granted is not asked: the refresh may only narrow the grant.
    async fn access_token_for(
        &self,
        audience: &Audience,
        kind: CapabilityKind,
    ) -> Result<IssuedToken, ProviderError> {
        let (family, form) = self.grant_target(audience, kind)?;
        let scopes = grant_scopes(kind, family);
        if scopes.is_empty() || !self.state().granted.holds_all(scopes) {
            return Err(ProviderError::Forbidden);
        }
        let token = self.mint_scoped(Some(&scopes.join(" "))).await?;
        self.shaped(token, form).await
    }

    fn renewed(&self) -> Option<Credential> {
        self.state().renewed.take()
    }
}
