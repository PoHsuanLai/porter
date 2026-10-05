//! The Microsoft family (lane W5b): personal and work or school accounts through one
//! multi-tenant public client. Sign-in is PKCE through a loopback redirect, or a device code;
//! mail is IMAP and SMTP with XOAUTH2 (tokens for Exchange Online); calendar, contacts, tasks,
//! OneNote and OneDrive are Graph, probed at add time so a tenant that forbids one shows it as
//! absent for consent (R13). Revoking has no API: the person is sent to account.microsoft.com.
//!
//! Everything it touches comes in through [`MicrosoftEnv`] (the `Http` seam, the client
//! registry, the clock, randomness), so tests drive it against a fake issuer.

mod env;
mod graph;
mod launcher;
mod scopes;
mod session;
mod signin;

pub use env::{Clock, MicrosoftEnv, Random, SignInFlow};
pub use scopes::{AccountClass, classify};
pub use session::MicrosoftSession;
pub use signin::MicrosoftSignIn;

use porter_core::capability::CapabilityKind;
use porter_core::{AccountId, Claim, EndpointUrl, Family};
use porter_http::{Http, HyperHttp};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSpec, RevokeOutcome, SignInStart,
};

/// Where a person ends access an app was given to their Microsoft account.
const APP_ACCESS_PAGE: &str = "https://account.microsoft.com/privacy/app-access";
/// Graph's origin, when the provider file names none.
const GRAPH_ORIGIN: &str = "https://graph.microsoft.com";
/// Exchange Online's IMAP server, when the provider file names none.
const IMAP_ORIGIN: &str = "imaps://outlook.office365.com:993";

/// The Microsoft family's provider, over the HTTP seam `H` (hyper's client in a daemon).
pub struct MicrosoftProvider<H = HyperHttp> {
    spec: ProviderSpec,
    env: MicrosoftEnv<H>,
}

impl<H> Clone for MicrosoftProvider<H> {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            env: self.env.clone(),
        }
    }
}

impl<H> std::fmt::Debug for MicrosoftProvider<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MicrosoftProvider")
            .field("spec", &self.spec.id)
            .field("env", &self.env)
            .finish()
    }
}

impl<H: Http + Default> MicrosoftProvider<H> {
    /// The provider serving the accounts of `spec`, in the system's environment.
    pub fn new(spec: ProviderSpec) -> Self {
        Self::with_env(spec, MicrosoftEnv::system())
    }
}

impl<H> MicrosoftProvider<H> {
    /// The provider serving the accounts of `spec` in `env`.
    pub fn with_env(spec: ProviderSpec, env: MicrosoftEnv<H>) -> Self {
        Self { spec, env }
    }
}

/// Graph's origin: the fixed endpoint of the file's Graph rows.
pub(crate) fn graph_origin(spec: &ProviderSpec) -> EndpointUrl {
    fixed_endpoint(spec, Family::Graph, GRAPH_ORIGIN)
}

/// Exchange Online's IMAP origin.
pub(crate) fn imap_origin(spec: &ProviderSpec) -> EndpointUrl {
    fixed_endpoint(spec, Family::Imap, IMAP_ORIGIN)
}

fn fixed_endpoint(spec: &ProviderSpec, family: Family, fallback: &str) -> EndpointUrl {
    spec.capabilities
        .iter()
        .filter(|row| row.family == family)
        .find_map(|row| EndpointUrl::parse(&row.endpoint.as_ref()?.0).ok())
        .or_else(|| EndpointUrl::parse(fallback).ok())
        .unwrap_or_else(|| unreachable!("{fallback} is a literal endpoint URL"))
}

/// The kinds the file declares, once each, in file order.
pub(crate) fn declared_kinds(spec: &ProviderSpec) -> Vec<CapabilityKind> {
    spec.capabilities
        .iter()
        .map(|row| row.capability.kind())
        .fold(Vec::new(), |mut kinds, kind| {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
            kinds
        })
}

impl<H: Http + 'static> Provider for MicrosoftProvider<H> {
    type Session = MicrosoftSession<H>;
    type SignIn = MicrosoftSignIn<H>;

    fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    async fn discover(
        &self,
        account: &AccountId,
        presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        let session = self.open(account, presented.clone()).await?;
        Ok(session.probe().await?.claims)
    }

    async fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> Result<MicrosoftSession<H>, ProviderError> {
        MicrosoftSession::new(
            account.clone(),
            self.spec.clone(),
            self.env.clone(),
            presented,
        )
    }

    fn sign_in(&self, start: SignInStart) -> Result<MicrosoftSignIn<H>, ProviderError> {
        Ok(MicrosoftSignIn::new(
            self.spec.clone(),
            self.env.clone(),
            start,
        ))
    }

    async fn revoke(&self, _presented: &Presented) -> Result<RevokeOutcome, ProviderError> {
        // Microsoft's v2 endpoint has no revoke; the account page lists and removes apps.
        Ok(EndpointUrl::parse(APP_ACCESS_PAGE)
            .map_or(RevokeOutcome::Unsupported, RevokeOutcome::Manual))
    }
}
