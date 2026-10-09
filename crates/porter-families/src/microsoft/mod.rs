//! The Microsoft family (lane W5b): personal and work or school accounts through one
//! multi-tenant public client. Sign-in is PKCE through a loopback redirect;
//! mail is IMAP and SMTP with XOAUTH2 (tokens for Exchange Online); calendar, contacts, tasks,
//! OneNote and OneDrive are Graph, probed at add time so a tenant that forbids one shows it as
//! absent for consent (R13). Revoking has no API: the person is sent to account.microsoft.com.
//!
//! Everything it touches comes in through [`MicrosoftEnv`] (the `Http` seam, the client
//! registry, the clock, randomness), so tests drive it against a fake issuer.

mod env;
mod graph;
mod scopes;
mod session;
mod signin;

pub use env::{Clock, MicrosoftEnv, Random};
pub use scopes::{AccountClass, classify};
pub use session::MicrosoftSession;
pub use signin::MicrosoftSignIn;

use porter_core::capability::CapabilityKind;
use porter_core::{Account, AccountId, Claim, Credential, EndpointUrl, Family, SecretText};
use porter_http::{Http, HyperHttp};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, Readiness, RevokeOutcome,
    SignInStart,
};
use std::collections::HashMap;

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
    rotations: Rotations,
}

/// Refresh tokens that `discover` rotated, by account: `(the token it was given, what replaced
/// it)`. `discover` cannot return a credential, so the next `open` of the account, which
/// presents the same old token, starts from the new one and reports it through `renewed`.
type Rotations = std::sync::Arc<std::sync::Mutex<HashMap<AccountId, (SecretText, Credential)>>>;

impl<H> Clone for MicrosoftProvider<H> {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            env: self.env.clone(),
            rotations: self.rotations.clone(),
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
        Self {
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
        account: &Account,
        presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        let session = self.open(&account.id, presented.clone()).await?;
        let found = session.probe().await?;
        if let (Some(renewed), Presented::Credential(Credential::OAuth { refresh, .. })) =
            (session.renewed(), presented)
        {
            self.rotations()
                .insert(account.id.clone(), (refresh.clone(), renewed));
        }
        Ok(found.claims)
    }

    async fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> Result<MicrosoftSession<H>, ProviderError> {
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
        let session = MicrosoftSession::new(
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

    fn sign_in(&self, start: SignInStart) -> Result<MicrosoftSignIn<H>, ProviderError> {
        Ok(MicrosoftSignIn::new(
            self.spec.clone(),
            self.env.clone(),
            start,
        ))
    }

    /// Ready while a Microsoft client is registered for this build's channel (shipped, or set
    /// in Settings: the files are read now).
    fn readiness(&self) -> Readiness {
        match self
            .env
            .clients()
            .lookup(porter_provider::Issuer::Microsoft, self.env.channel)
        {
            Some(_) => Readiness::Ready,
            None => Readiness::NeedsClient,
        }
    }

    async fn revoke(
        &self,
        _account: &Account,
        _presented: &Presented,
    ) -> Result<RevokeOutcome, ProviderError> {
        // Microsoft's v2 endpoint has no revoke; the account page lists and removes apps.
        Ok(EndpointUrl::parse(APP_ACCESS_PAGE)
            .map_or(RevokeOutcome::Unsupported, RevokeOutcome::Manual))
    }
}
