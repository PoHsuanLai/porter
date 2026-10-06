//! The Generic family: a mail server or a DAV server with a password (or an app password) and
//! no provider of its own. `generic-imap` is found by autoconfig, SRV and MX from the address,
//! `generic-dav` by `.well-known` from the server's name, and a brand file with fixed endpoints
//! (`fastmail`, `icloud`, `yahoo`, `gmx`) asks for an address and a password and uses the servers
//! the file names.
//!
//! Like Nextcloud's, a session mints no token (the relay presents the password), `discover`
//! answers what the provider file declares, and `revoke` has nothing to do at a server that
//! has no such call: removing the account wipes the password locally.

mod dav;
mod jmap;
mod mail;
mod sign_in;

use crate::io::{Io, SharedDns};
use crate::password::{declared, secret_presented};
use porter_core::{Account, AccountId, Audience, Claim, Credential, IssuedToken};
use porter_discover::Dns;
use porter_http::{NoSleep, SharedHttp};
use porter_provider::{
    Discovery, Presented, Provider, ProviderError, ProviderSession, ProviderSet, ProviderSpec,
    RevokeOutcome, SignInStart,
};

pub use sign_in::GenericSignIn;

/// The Generic family's provider.
#[derive(Debug, Clone)]
pub struct GenericProvider {
    spec: ProviderSpec,
    io: Io,
    dns: SharedDns,
    providers: ProviderSet,
}

impl GenericProvider {
    /// The provider serving the accounts of `spec`, dialling through `http` and resolving
    /// through `dns`. It knows no other provider, so every address is searched for.
    pub fn new(spec: ProviderSpec, http: SharedHttp, dns: impl Dns + 'static) -> Self {
        Self {
            spec,
            io: Io::new(http, NoSleep),
            dns: SharedDns::new(dns),
            providers: ProviderSet::default(),
        }
    }

    /// The same provider that lets these providers claim an address first (a Fastmail address
    /// is Fastmail's to sign in).
    pub fn with_providers(self, providers: ProviderSet) -> Self {
        Self { providers, ..self }
    }
}

/// An open generic account.
#[derive(Debug)]
pub struct GenericSession {
    account: AccountId,
}

impl GenericSession {
    /// The account it is open for.
    pub fn account(&self) -> &AccountId {
        &self.account
    }
}

impl Provider for GenericProvider {
    type Session = GenericSession;
    type SignIn = GenericSignIn;

    fn spec(&self) -> &ProviderSpec {
        &self.spec
    }

    async fn discover(
        &self,
        _account: &Account,
        _presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        Ok(declared(&self.spec))
    }

    async fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> Result<GenericSession, ProviderError> {
        secret_presented(&presented)?;
        Ok(GenericSession {
            account: account.clone(),
        })
    }

    fn sign_in(&self, start: SignInStart) -> Result<GenericSignIn, ProviderError> {
        let flavor = match self.spec.discovery {
            Discovery::Autoconfig => sign_in::Flavor::Mail,
            Discovery::WellKnown => sign_in::Flavor::Dav,
            Discovery::Fixed => sign_in::Flavor::Fixed,
            _ => return Err(ProviderError::Unreadable),
        };
        Ok(GenericSignIn::new(
            self.io.clone(),
            self.dns.clone(),
            self.providers.clone(),
            self.spec.clone(),
            flavor,
            start.mode,
        ))
    }

    async fn revoke(
        &self,
        _account: &Account,
        presented: &Presented,
    ) -> Result<RevokeOutcome, ProviderError> {
        secret_presented(presented)?;
        Ok(RevokeOutcome::Unsupported)
    }
}

impl ProviderSession for GenericSession {
    async fn access_token(&self, _audience: &Audience) -> Result<IssuedToken, ProviderError> {
        // A password is never handed to an app; the relay presents it.
        Err(ProviderError::Forbidden)
    }

    fn renewed(&self) -> Option<Credential> {
        None
    }
}
