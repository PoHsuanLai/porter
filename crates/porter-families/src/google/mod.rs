//! The Google family (lane W5c): the owner's own Google account through a Google Cloud OAuth
//! client the owner registers (docs/google.md). Sign-in is PKCE through a loopback redirect (the
//! installed-app flow) asking offline access and one set of scopes per capability; Calendar,
//! People, Tasks and the Drive app folder are probed at add time, Photos is shown by its scope,
//! and Gmail over IMAP and SMTP with XOAUTH2 is there only for a client that is the person's own
//! (`byo = true` in its row), since its scope is restricted.
//!
//! With no client row the provider is still served: signing in answers `NeedsClientId`, which the
//! sheet words as "Google sign-in needs a client id" and points at the guide. A client whose row
//! says `testing = true` is signed out by Google every seven days; a refresh Google refuses is
//! then `NeedsReauth` with [`ReauthReason::TestingExpiry`].
//!
//! Everything it touches comes in through [`GoogleEnv`] (the `Http` seam, the client registry,
//! the clock, randomness), so tests drive it against a fake Google.

mod env;
mod probe;
mod reauth;
mod scopes;
mod session;
mod signin;

pub use env::GoogleEnv;
pub use reauth::{ReauthReason, TESTING_SIGN_IN_SECONDS, testing_expires};
pub use scopes::{Sensitivity, scope_sensitivity, scopes_of};
pub use session::GoogleSession;
pub use signin::GoogleSignIn;

use porter_core::capability::CapabilityKind;
use porter_core::{Account, AccountId, Claim, Credential, EndpointUrl, Family};
use porter_http::{Http, HyperHttp};
use porter_oauth::{ExchangeFault, endpoints_of, revoke};
use porter_provider::{
    Issuer, Presented, Provider, ProviderError, ProviderSpec, Readiness, RevokeOutcome, SignInStart,
};

/// The Google family's provider, over the HTTP seam `H` (hyper's client in a daemon).
pub struct GoogleProvider<H = HyperHttp> {
    spec: ProviderSpec,
    env: GoogleEnv<H>,
}

impl<H> Clone for GoogleProvider<H> {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            env: self.env.clone(),
        }
    }
}

impl<H> std::fmt::Debug for GoogleProvider<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleProvider")
            .field("spec", &self.spec.id)
            .field("env", &self.env)
            .finish()
    }
}

impl<H: Http + Default> GoogleProvider<H> {
    /// The provider serving the accounts of `spec`, in the system's environment.
    pub fn new(spec: ProviderSpec) -> Self {
        Self::with_env(spec, GoogleEnv::system())
    }
}

impl<H> GoogleProvider<H> {
    /// The provider serving the accounts of `spec` in `env`.
    pub fn with_env(spec: ProviderSpec, env: GoogleEnv<H>) -> Self {
        Self { spec, env }
    }
}

/// The APIs' bases: the fixed endpoints of the file's rows, Google's own where it names none.
#[derive(Debug, Clone)]
pub(crate) struct Apis(Vec<(Family, EndpointUrl)>);

impl Apis {
    /// The base of `family`'s API.
    pub(crate) fn base(&self, family: Family) -> EndpointUrl {
        self.0
            .iter()
            .find(|(f, _)| *f == family)
            .map(|(_, url)| url.clone())
            .or_else(|| fallback(family))
            .unwrap_or_else(|| unreachable!("{family:?} is a Google API family"))
    }
}

/// The API origins when the provider file names none.
fn fallback(family: Family) -> Option<EndpointUrl> {
    let url = match family {
        Family::GoogleCalendar => "https://www.googleapis.com/calendar/v3",
        Family::GooglePeople => "https://people.googleapis.com/v1",
        Family::GoogleTasks => "https://tasks.googleapis.com/tasks/v1",
        Family::GoogleDrive => "https://www.googleapis.com/drive/v3",
        Family::GooglePhotosUpload => "https://photoslibrary.googleapis.com/v1",
        Family::GooglePhotosPicker => "https://photospicker.googleapis.com/v1",
        Family::Imap => "imaps://imap.gmail.com:993",
        _ => return None,
    };
    EndpointUrl::parse(url).ok()
}

/// The endpoints the file's rows name.
pub(crate) fn apis_of(spec: &ProviderSpec) -> Apis {
    Apis(
        spec.capabilities
            .iter()
            .filter_map(|row| {
                let url = EndpointUrl::parse(&row.endpoint.as_ref()?.0).ok()?;
                Some((row.family, url))
            })
            .collect(),
    )
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

impl<H: Http + 'static> Provider for GoogleProvider<H> {
    type Session = GoogleSession<H>;
    type SignIn = GoogleSignIn<H>;

    fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    async fn discover(
        &self,
        account: &Account,
        presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        let session = self.open(&account.id, presented.clone()).await?;
        session.probe().await
    }

    async fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> Result<GoogleSession<H>, ProviderError> {
        GoogleSession::new(
            account.clone(),
            self.spec.clone(),
            self.env.clone(),
            presented,
        )
    }

    fn sign_in(&self, start: SignInStart) -> Result<GoogleSignIn<H>, ProviderError> {
        Ok(GoogleSignIn::new(
            self.spec.clone(),
            self.env.clone(),
            start,
        ))
    }

    /// Ready while a Google client is registered for this build's channel (none is shipped: the
    /// person sets one in Settings, and the files are read now).
    fn readiness(&self) -> Readiness {
        match self.env.clients().lookup(Issuer::Google, self.env.channel) {
            Some(_) => Readiness::Ready,
            None => Readiness::NeedsClient,
        }
    }

    /// Revokes the refresh token at Google's revoke endpoint, which ends the access tokens made
    /// from it too. A token Google no longer knows is revoked already.
    async fn revoke(
        &self,
        _account: &Account,
        presented: &Presented,
    ) -> Result<RevokeOutcome, ProviderError> {
        let Presented::Credential(Credential::OAuth { refresh, .. }) = presented else {
            return Ok(RevokeOutcome::Unsupported);
        };
        let clients = self.env.clients().into_owned();
        let Some(client) = clients.lookup(Issuer::Google, self.env.channel) else {
            // No client is configured: nothing can be asked of Google, and the account's own
            // secrets are deleted regardless.
            return Ok(RevokeOutcome::Unsupported);
        };
        match revoke(&*self.env.http, &endpoints_of(client), refresh).await {
            Ok(()) | Err(ExchangeFault::Refused) => Ok(RevokeOutcome::Revoked),
            Err(ExchangeFault::Unreachable) => Err(ProviderError::Unreachable),
            Err(ExchangeFault::Unreadable) => Ok(RevokeOutcome::Unsupported),
        }
    }
}
