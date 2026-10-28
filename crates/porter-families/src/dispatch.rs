//! The object-safe twins of `Provider`, `ProviderSession` and `SignIn`, and the one impl of each
//! trait over them. A family is chosen at run time from a provider file, so the host holds
//! `Box<dyn DynProvider>`; the blanket impls here are the only place that boxes, the same way
//! `SharedHttp` boxes an `Http`.

use porter_capability::StorageScope;
use porter_capability::{CapabilityKind, Claim};
use porter_core::AccountId;
use porter_core::{Account, Audience, Credential, IssuedToken};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, Readiness, RevokeOutcome,
    SignIn, SignInStart, SignInStep,
};
use porter_sheet::SignInInput;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A [`Provider`] that can be held as a trait object: the host keeps one per provider file as a
/// `Box<dyn DynProvider>`, which is itself a `Provider`.
pub trait DynProvider: Debug + Send + Sync {
    /// [`Provider::spec`].
    fn spec(&self) -> &ProviderSpec;

    /// [`Provider::discover`].
    fn discover<'a>(
        &'a self,
        account: &'a Account,
        presented: &'a Presented,
    ) -> BoxFut<'a, Result<Vec<Claim>, ProviderError>>;

    /// [`Provider::open`].
    fn open<'a>(
        &'a self,
        account: &'a AccountId,
        presented: Presented,
    ) -> BoxFut<'a, Result<Box<dyn DynSession>, ProviderError>>;

    /// [`Provider::sign_in`].
    ///
    /// # Errors
    /// What [`Provider::sign_in`] refuses with.
    fn sign_in(&self, start: SignInStart) -> Result<Box<dyn DynSignIn>, ProviderError>;

    /// [`Provider::readiness`].
    fn readiness(&self) -> Readiness;

    /// [`Provider::revoke`].
    fn revoke<'a>(
        &'a self,
        account: &'a Account,
        presented: &'a Presented,
    ) -> BoxFut<'a, Result<RevokeOutcome, ProviderError>>;

    /// This provider, boxed.
    fn boxed(self) -> Box<dyn DynProvider>
    where
        Self: Sized + 'static,
    {
        Box::new(self)
    }
}

/// A [`ProviderSession`] that can be held as a trait object.
pub trait DynSession: Debug + Send + Sync {
    /// [`ProviderSession::access_token`].
    fn access_token<'a>(
        &'a self,
        audience: &'a Audience,
    ) -> BoxFut<'a, Result<IssuedToken, ProviderError>>;

    /// [`ProviderSession::access_token_for`].
    fn access_token_for<'a>(
        &'a self,
        audience: &'a Audience,
        kind: CapabilityKind,
        reach: StorageScope,
    ) -> BoxFut<'a, Result<IssuedToken, ProviderError>>;

    /// [`ProviderSession::renewed`].
    fn renewed(&self) -> Option<Credential>;
}

/// A [`SignIn`] that can be held as a trait object.
pub trait DynSignIn: Debug + Send {
    /// [`SignIn::next`].
    fn next(&mut self, input: SignInInput) -> BoxFut<'_, SignInStep>;
}

impl<P: Provider + Debug + 'static> DynProvider for P
where
    P::Session: Debug + 'static,
    P::SignIn: Debug + 'static,
{
    fn spec(&self) -> &ProviderSpec {
        Provider::spec(self)
    }

    fn discover<'a>(
        &'a self,
        account: &'a Account,
        presented: &'a Presented,
    ) -> BoxFut<'a, Result<Vec<Claim>, ProviderError>> {
        Box::pin(Provider::discover(self, account, presented))
    }

    fn open<'a>(
        &'a self,
        account: &'a AccountId,
        presented: Presented,
    ) -> BoxFut<'a, Result<Box<dyn DynSession>, ProviderError>> {
        Box::pin(async move {
            Provider::open(self, account, presented)
                .await
                .map(|session| Box::new(session) as Box<dyn DynSession>)
        })
    }

    fn sign_in(&self, start: SignInStart) -> Result<Box<dyn DynSignIn>, ProviderError> {
        Provider::sign_in(self, start).map(|sign_in| Box::new(sign_in) as Box<dyn DynSignIn>)
    }

    fn readiness(&self) -> Readiness {
        Provider::readiness(self)
    }

    fn revoke<'a>(
        &'a self,
        account: &'a Account,
        presented: &'a Presented,
    ) -> BoxFut<'a, Result<RevokeOutcome, ProviderError>> {
        Box::pin(Provider::revoke(self, account, presented))
    }
}

impl<S: ProviderSession + Debug> DynSession for S {
    fn access_token<'a>(
        &'a self,
        audience: &'a Audience,
    ) -> BoxFut<'a, Result<IssuedToken, ProviderError>> {
        Box::pin(ProviderSession::access_token(self, audience))
    }

    fn access_token_for<'a>(
        &'a self,
        audience: &'a Audience,
        kind: CapabilityKind,
        reach: StorageScope,
    ) -> BoxFut<'a, Result<IssuedToken, ProviderError>> {
        Box::pin(ProviderSession::access_token_for(
            self, audience, kind, reach,
        ))
    }

    fn renewed(&self) -> Option<Credential> {
        ProviderSession::renewed(self)
    }
}

impl<S: SignIn + Debug> DynSignIn for S {
    fn next(&mut self, input: SignInInput) -> BoxFut<'_, SignInStep> {
        Box::pin(SignIn::next(self, input))
    }
}

impl Provider for Box<dyn DynProvider> {
    type Session = Box<dyn DynSession>;
    type SignIn = Box<dyn DynSignIn>;

    fn spec(&self) -> &ProviderSpec {
        DynProvider::spec(&**self)
    }

    async fn discover(
        &self,
        account: &Account,
        presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        DynProvider::discover(&**self, account, presented).await
    }

    async fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> Result<Box<dyn DynSession>, ProviderError> {
        DynProvider::open(&**self, account, presented).await
    }

    fn sign_in(&self, start: SignInStart) -> Result<Box<dyn DynSignIn>, ProviderError> {
        DynProvider::sign_in(&**self, start)
    }

    fn readiness(&self) -> Readiness {
        DynProvider::readiness(&**self)
    }

    async fn revoke(
        &self,
        account: &Account,
        presented: &Presented,
    ) -> Result<RevokeOutcome, ProviderError> {
        DynProvider::revoke(&**self, account, presented).await
    }
}

impl ProviderSession for Box<dyn DynSession> {
    async fn access_token(&self, audience: &Audience) -> Result<IssuedToken, ProviderError> {
        DynSession::access_token(&**self, audience).await
    }

    async fn access_token_for(
        &self,
        audience: &Audience,
        kind: CapabilityKind,
        reach: StorageScope,
    ) -> Result<IssuedToken, ProviderError> {
        DynSession::access_token_for(&**self, audience, kind, reach).await
    }

    fn renewed(&self) -> Option<Credential> {
        DynSession::renewed(&**self)
    }
}

impl SignIn for Box<dyn DynSignIn> {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        DynSignIn::next(&mut **self, input).await
    }
}
