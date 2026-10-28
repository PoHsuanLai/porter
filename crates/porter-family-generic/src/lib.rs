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
mod dns;
mod jmap;
mod login;
mod lookup;
mod mail;
mod sign_in;

/// What a generic-proto error means for a sign-in. Plain `http` to another computer is a
/// server that cannot be reached safely.
fn fault(error: generic_proto::Error) -> porter_sheet::SignInFault {
    use porter_sheet::SignInFault;
    match error {
        generic_proto::Error::Refused => SignInFault::Refused,
        generic_proto::Error::Unreachable | generic_proto::Error::Insecure => {
            SignInFault::Unreachable
        }
        _ => SignInFault::Unreadable,
    }
}

use generic_proto::declared;
use porter_capability::Claim;
use porter_core::AccountId;
use porter_core::{Account, Audience, Credential, IssuedToken};
use porter_discover::Dns;
use porter_family_kit::Conversation;
use porter_family_kit::Io;
use porter_family_kit::password::secret_presented;
use porter_http::{NoSleep, SharedHttp};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSet, ProviderSpec, RevokeOutcome,
    SignInStart,
};
use std::marker::PhantomData;

pub use dns::SharedDns;
pub use lookup::{DavLookup, FixedLookup, MailLookup};
pub use sign_in::{GenericFlow, Located, Lookup, Mid, Outcome, Seams};

/// The Generic family's provider for the accounts `L` finds the servers of.
#[derive(Debug, Clone)]
pub struct GenericProvider<L = MailLookup> {
    spec: ProviderSpec,
    io: Io,
    dns: SharedDns,
    providers: ProviderSet,
    login: login::SharedLogin,
    lookup: PhantomData<fn() -> L>,
}

impl<L> GenericProvider<L> {
    /// The provider serving the accounts of `spec`, dialling through `http` and resolving
    /// through `dns`. It knows no other provider, so every address is searched for.
    pub fn new(spec: ProviderSpec, http: SharedHttp, dns: impl Dns + 'static) -> Self {
        Self {
            spec,
            io: Io::new(http, NoSleep),
            dns: SharedDns::new(dns),
            providers: ProviderSet::default(),
            login: login::SharedLogin::platform(),
            lookup: PhantomData,
        }
    }

    /// The same provider that tries a typed password at the mail server through `connect`
    /// (default: TCP and TLS over the platform's roots). A test's is a connector that trusts a
    /// scratch CA.
    pub fn with_connect(self, connect: impl porter_proxy::Connect + 'static) -> Self {
        Self {
            login: login::SharedLogin::new(connect),
            ..self
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

impl<L: Lookup> Provider for GenericProvider<L> {
    type Session = GenericSession;
    type SignIn = Conversation<GenericFlow<L>>;

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

    fn sign_in(&self, start: SignInStart) -> Result<Self::SignIn, ProviderError> {
        Ok(Conversation::new(GenericFlow::new(
            self.io.clone(),
            self.dns.clone(),
            self.providers.clone(),
            self.login.clone(),
            self.spec.clone(),
            start.mode,
        )))
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
