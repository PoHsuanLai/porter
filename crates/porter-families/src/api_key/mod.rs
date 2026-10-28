//! The ApiKey family (lane AI2a): an AI company whose key the person pastes (Anthropic, Google
//! Gemini, Moonshot, OpenAI; any file with `kind = "api_key"` and an Llm row). Sign-in asks for the key in a hidden
//! field, lists the provider's models with it as the check, and files the key as the account's
//! one secret. The key never leaves accountd: a session mints no token, and `Peer.ResolveKey`
//! gives it to a porter daemon on a sealed memfd.
//!
//! Revoking is the person's: the family has no key-delete call, so the company's keys page is named.
//!
//! A file that names no server (`openai-compatible`) is a server of the person's own: the sign-in
//! asks for its address and an optional key, the address is kept with the account, and a later
//! check (`discover`) asks that server.

mod sign_in;

use crate::key::{asks_server, check, check_server, key_of, own_server};
use porter_capability::Claim;
use porter_core::AccountId;
use porter_core::{Account, EndpointUrl};
use porter_http::Http;
use porter_http_reqwest::ReqwestHttp;
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSpec, RevokeOutcome, SignInStart,
};
use std::sync::Arc;

pub use crate::key::KeySession as ApiKeySession;
pub use sign_in::ApiKeySignIn;

/// Where a person deletes a key, by provider file id.
const KEY_PAGES: &[(&str, &str)] = &[
    ("anthropic", "https://console.anthropic.com/settings/keys"),
    ("google-ai", "https://aistudio.google.com/apikey"),
    ("moonshot", "https://platform.moonshot.ai/console/api-keys"),
    ("openai", "https://platform.openai.com/api-keys"),
    ("openrouter", "https://openrouter.ai/settings/keys"),
];

/// The ApiKey family's provider, over the HTTP seam `H` (hyper's client in a daemon).
pub struct ApiKeyProvider<H = ReqwestHttp> {
    spec: ProviderSpec,
    http: Arc<H>,
}

impl<H> Clone for ApiKeyProvider<H> {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            http: Arc::clone(&self.http),
        }
    }
}

impl<H> std::fmt::Debug for ApiKeyProvider<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeyProvider")
            .field("spec", &self.spec.id)
            .finish_non_exhaustive()
    }
}

impl<H: Http + Default> ApiKeyProvider<H> {
    /// The provider serving the accounts of `spec`, dialling with the default client.
    pub fn new(spec: ProviderSpec) -> Self {
        Self::with_http(spec, H::default())
    }
}

impl<H> ApiKeyProvider<H> {
    /// The provider serving the accounts of `spec`, dialling through `http`.
    pub fn with_http(spec: ProviderSpec, http: H) -> Self {
        Self {
            spec,
            http: Arc::new(http),
        }
    }
}

impl<H: Http + 'static> Provider for ApiKeyProvider<H> {
    type Session = ApiKeySession;
    type SignIn = ApiKeySignIn<H>;

    fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    async fn discover(
        &self,
        account: &Account,
        presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        let key = key_of(presented)?;
        match asks_server(&self.spec) {
            true => {
                let server = own_server(&account.endpoints).ok_or(ProviderError::Unreadable)?;
                check_server(&*self.http, &server.url, key).await?;
            }
            false => check(&*self.http, &self.spec, key).await?,
        }
        Ok(crate::key::claims(&self.spec))
    }

    async fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> Result<ApiKeySession, ProviderError> {
        key_of(&presented)?;
        Ok(ApiKeySession::new(account.clone()))
    }

    fn sign_in(&self, start: SignInStart) -> Result<ApiKeySignIn<H>, ProviderError> {
        Ok(ApiKeySignIn::new(
            self.spec.clone(),
            Arc::clone(&self.http),
            start.mode,
        ))
    }

    async fn revoke(
        &self,
        _account: &Account,
        _presented: &Presented,
    ) -> Result<RevokeOutcome, ProviderError> {
        Ok(KEY_PAGES
            .iter()
            .find(|(id, _)| *id == self.spec.id.as_str())
            .and_then(|(_, page)| EndpointUrl::parse(page).ok())
            .map_or(RevokeOutcome::Unsupported, RevokeOutcome::Manual))
    }
}
