//! What differs between OAuth issuers. Everything an issuer publishes (endpoints, the scopes it
//! knows and for which capability kind, who answered at userinfo) is data (`issuers.toml`, read
//! through [`porter_provider::Issuer::data`]); what is left is the few rules that data cannot
//! say, written once per issuer as an impl of [`BrowserIssuer`]. A provider file whose issuer
//! publishes a userinfo endpoint and has no rules of its own is served by `Standard`, with no
//! Rust added for it.

use super::flow::{Cx, Grant};
use super::session::OAuthSession;
use porter_capability::{CapabilityKind, Claim, StorageScope};
use porter_core::{Audience, Credential, GrantedScopes, ServiceEndpoint, TokenKind};
use porter_http::Http;
use porter_oauth::TokenResponse;
use porter_provider::{Issuer, ProviderError, ProviderSpec, SignInMode, Signed};
use porter_sheet::SignInFault;
use std::fmt::Debug;
use std::future::Future;

/// One OAuth issuer's rules: the provider, the sign-in and the session each hold a copy of them.
/// A unit value for an issuer with rules of its own, `Standard` for one without.
pub trait BrowserIssuer: Clone + Debug + Send + Sync + Sized + 'static {
    /// The issuer, whose data (`issuers.toml`) and client row these rules are for.
    fn issuer(&self) -> Issuer;

    /// What a session of this issuer keeps beside its tokens.
    type State: Debug + Send + Sync + 'static;

    /// The state a session opens with, from the credential it was handed.
    fn open_state(&self, credential: &Credential) -> Self::State;

    /// The scopes a sign-in asks: the data's, for the kinds the file signs in with (or, for one
    /// more access, that access alone). `byo` is whether the client is the person's own.
    fn scopes(&self, spec: &ProviderSpec, mode: &SignInMode, byo: bool) -> Vec<String> {
        let data = self.issuer().data();
        match mode {
            SignInMode::Extend { kinds, .. } => data.extend_scopes(kinds),
            _ => data.sign_in_scopes(&spec.sign_in_kinds(), byo),
        }
    }

    /// The scopes a code is redeemed for when the sign-in spans more than one resource. The
    /// default redeems it whole.
    fn redeem_scope(&self, scopes: &[String]) -> Option<String> {
        let _ = scopes;
        None
    }

    /// What the issuer's redirect (`error=`) says of a sign-in that did not go through. A person
    /// who said no cancelled; the rest are refused unless an issuer's rules say otherwise.
    fn refusal(&self, error: &str) -> SignInFault {
        match error {
            "access_denied" => SignInFault::Cancelled,
            _ => SignInFault::Refused,
        }
    }

    /// The tokens are in: read the account and make what the person reviews (a new account) or
    /// signs in again with.
    fn signed<H: Http + 'static>(
        &self,
        cx: &Cx<'_, H>,
        grant: Grant,
        tokens: TokenResponse,
    ) -> impl Future<Output = Result<Signed, SignInFault>> + Send;

    /// The tokens of a sign-in that asked for the on-use access of `kinds` alone: who answered
    /// (the host keeps the answer only for the account's own login) and a review of that access.
    fn extended<H: Http + 'static>(
        &self,
        cx: &Cx<'_, H>,
        grant: &Grant,
        tokens: TokenResponse,
        stored: &[ServiceEndpoint],
        kinds: &[CapabilityKind],
    ) -> impl Future<Output = Result<Signed, SignInFault>> + Send;

    /// What the account's services answer.
    fn discover<H: Http + 'static>(
        &self,
        session: &OAuthSession<Self, H>,
    ) -> impl Future<Output = Result<Vec<Claim>, ProviderError>> + Send;

    /// The address the account signs in with (read once; XOAUTH2 names the user).
    fn identify<H: Http + 'static>(
        &self,
        session: &OAuthSession<Self, H>,
    ) -> impl Future<Output = Result<String, ProviderError>> + Send;

    /// What porter's own token for `audience` is refreshed with (`None`: the whole grant) and
    /// its form, or `Forbidden` for an audience that is not the account's. No token minted this
    /// way leaves accountd: an app's or a relay's comes from [`BrowserIssuer::grant_token`].
    fn porter_token<H>(
        &self,
        session: &OAuthSession<Self, H>,
        audience: &Audience,
    ) -> Result<(Option<String>, TokenKind), ProviderError>;

    /// What a token for a grant of `kind` to `audience` is refreshed with and its form, or why
    /// the grant is refused.
    fn grant_token<H>(
        &self,
        session: &OAuthSession<Self, H>,
        audience: &Audience,
        kind: CapabilityKind,
        reach: StorageScope,
    ) -> Result<(Option<String>, TokenKind), ProviderError>;

    /// What the credential records as granted after `tokens` answered a refresh for `scope`
    /// (`None`: the whole grant). The default is that nothing changed: `current`.
    fn answered(
        &self,
        state: &mut Self::State,
        current: &GrantedScopes,
        scope: Option<&str>,
        tokens: &TokenResponse,
    ) -> GrantedScopes {
        let _ = (state, scope, tokens);
        current.clone()
    }

    /// A refresh was refused (`invalid_grant`): note why, if the issuer's rules know.
    fn refused(&self, state: &mut Self::State, testing: bool, now: porter_ids::UnixSeconds) {
        let _ = (state, testing, now);
    }
}
