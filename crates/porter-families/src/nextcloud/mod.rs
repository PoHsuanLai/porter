//! The Nextcloud family: Login Flow v2 (a browser sign-in that mints an app password), OCS
//! discovery, WebDAV, CalDAV, CardDAV and the Notes API, over one app password.
//!
//! A password never leaves accountd, so a session mints no token; apps reach the servers through
//! the authenticated relay. The trait's `discover`, `open` and `revoke` are handed an account id
//! and a credential and not the account's server, which a Nextcloud needs (FINDINGS): `open`
//! needs none, `discover` answers what the provider file declares, and the real calls are
//! [`NextcloudProvider::discover_at`] and [`NextcloudProvider::revoke_at`], which take the
//! account's endpoint.

mod discover;
mod flow;
mod revoke;
mod sign_in;

use crate::io::{Io, Pacing};
use crate::password::{declared, password_of};
use discover::Who;
use porter_core::{
    AccountId, Audience, Claim, Credential, EndpointUrl, IssuedToken, LoginName, SecretText,
    ServiceEndpoint,
};
use porter_http::{SharedHttp, Sleep};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, RevokeOutcome, SignInStart,
};

pub use sign_in::NextcloudSignIn;

/// The Nextcloud family's provider.
#[derive(Debug, Clone)]
pub struct NextcloudProvider {
    spec: ProviderSpec,
    io: Io,
}

impl NextcloudProvider {
    /// The provider serving the accounts of `spec`, dialling through `http` and waiting on
    /// `sleep` between the polls of a sign-in.
    pub fn new(spec: ProviderSpec, http: SharedHttp, sleep: impl Sleep + 'static) -> Self {
        Self {
            spec,
            io: Io::new(http, sleep),
        }
    }

    /// The same provider polling at this pace.
    pub fn with_pacing(self, pacing: Pacing) -> Self {
        Self {
            io: Io { pacing, ..self.io },
            ..self
        }
    }

    /// Where the provider file says the Nextcloud is, if it names one: the root of its DAV
    /// endpoint rows (a deployment's own file; a test's fake).
    fn fixed_server(&self) -> Option<EndpointUrl> {
        self.spec.capabilities.iter().find_map(|row| {
            let at = row.endpoint.as_ref()?;
            let root = at.0.split("/remote.php/").next()?;
            (root.len() < at.0.len())
                .then(|| EndpointUrl::parse(root.trim_end_matches('/')).ok())
                .flatten()
        })
    }

    /// What the account at `server` can do now: the OCS capabilities, the DAV principal and the
    /// homes. A refused password is `Unauthorized`.
    pub async fn discover_at(
        &self,
        server: &EndpointUrl,
        login: &LoginName,
        password: &SecretText,
    ) -> Result<(Vec<Claim>, Vec<ServiceEndpoint>), ProviderError> {
        let who = Who {
            server: server.clone(),
            login: login.clone(),
            password: password.clone(),
        };
        discover::discover(&self.io, &who)
            .await
            .map(|found| (found.claims, found.endpoints))
            .map_err(|fault| match fault {
                porter_core::sheet::SignInFault::Refused => ProviderError::Unauthorized,
                porter_core::sheet::SignInFault::Unreachable => ProviderError::Unreachable,
                _ => ProviderError::Unreadable,
            })
    }

    /// Deletes the app password at `server` (OCS `DELETE /ocs/v2.php/core/apppassword`).
    pub async fn revoke_at(
        &self,
        server: &EndpointUrl,
        login: &LoginName,
        password: &SecretText,
    ) -> Result<RevokeOutcome, ProviderError> {
        revoke::revoke(&self.io, server, login, password).await
    }
}

/// An open Nextcloud account.
#[derive(Debug)]
pub struct NextcloudSession {
    account: AccountId,
}

impl NextcloudSession {
    /// The account it is open for.
    pub fn account(&self) -> &AccountId {
        &self.account
    }
}

impl Provider for NextcloudProvider {
    type Session = NextcloudSession;
    type SignIn = NextcloudSignIn;

    fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    async fn discover(
        &self,
        _account: &AccountId,
        _presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        Ok(declared(&self.spec))
    }

    async fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> Result<NextcloudSession, ProviderError> {
        password_of(&presented)?;
        Ok(NextcloudSession {
            account: account.clone(),
        })
    }

    fn sign_in(&self, start: SignInStart) -> Result<NextcloudSignIn, ProviderError> {
        Ok(NextcloudSignIn::new(
            self.io.clone(),
            start.mode,
            self.fixed_server(),
        ))
    }

    async fn revoke(&self, presented: &Presented) -> Result<RevokeOutcome, ProviderError> {
        // The server and the login name are the account's, which the trait does not pass.
        password_of(presented)?;
        Ok(RevokeOutcome::Unsupported)
    }
}

impl ProviderSession for NextcloudSession {
    async fn access_token(&self, _audience: &Audience) -> Result<IssuedToken, ProviderError> {
        // An app password is never handed to an app; the relay presents it.
        Err(ProviderError::Forbidden)
    }

    fn renewed(&self) -> Option<Credential> {
        None
    }
}
