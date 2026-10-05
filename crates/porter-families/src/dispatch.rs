//! The closed enums over the built families: a variant per family, its arms present only when
//! its feature is on.

use porter_core::sheet::SignInInput;
use porter_core::{AccountId, Audience, Claim, Credential, IssuedToken};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, RevokeOutcome, SignIn,
    SignInStart, SignInStep,
};

/// Expands `$body` once per built family, binding the variant's payload to `$p`.
macro_rules! each_family {
    ($value:expr, $p:ident => $body:expr) => {
        match $value {
            #[cfg(feature = "nextcloud")]
            Self::Nextcloud($p) => $body,
            #[cfg(feature = "generic")]
            Self::Generic($p) => $body,
            #[cfg(feature = "microsoft")]
            Self::Microsoft($p) => $body,
            #[cfg(feature = "api_key")]
            Self::ApiKey($p) => $body,
            #[cfg(feature = "openrouter")]
            Self::OpenRouter($p) => $body,
            // Present for the build with no family, where the enum has no variant to match.
            #[allow(unreachable_patterns)]
            _ => unreachable!("no family is built"),
        }
    };
}

/// Every built family's provider.
#[derive(Debug)]
pub enum FamilyProvider {
    /// Nextcloud.
    #[cfg(feature = "nextcloud")]
    Nextcloud(crate::NextcloudProvider),
    /// Generic IMAP/SMTP and DAV.
    #[cfg(feature = "generic")]
    Generic(crate::GenericProvider),
    /// Microsoft.
    #[cfg(feature = "microsoft")]
    Microsoft(crate::MicrosoftProvider),
    /// Pasted API keys.
    #[cfg(feature = "api_key")]
    ApiKey(crate::ApiKeyProvider),
    /// OpenRouter.
    #[cfg(feature = "openrouter")]
    OpenRouter(crate::OpenRouterProvider),
}

/// An open account of a built family.
#[derive(Debug)]
pub enum FamilySession {
    /// Nextcloud.
    #[cfg(feature = "nextcloud")]
    Nextcloud(crate::NextcloudSession),
    /// Generic IMAP/SMTP and DAV.
    #[cfg(feature = "generic")]
    Generic(crate::GenericSession),
    /// Microsoft.
    #[cfg(feature = "microsoft")]
    Microsoft(crate::MicrosoftSession),
    /// Pasted API keys.
    #[cfg(feature = "api_key")]
    ApiKey(crate::ApiKeySession),
    /// OpenRouter.
    #[cfg(feature = "openrouter")]
    OpenRouter(crate::OpenRouterSession),
}

/// A sign-in conversation of a built family.
#[derive(Debug)]
pub enum FamilySignIn {
    /// Nextcloud.
    #[cfg(feature = "nextcloud")]
    Nextcloud(crate::NextcloudSignIn),
    /// Generic IMAP/SMTP and DAV.
    #[cfg(feature = "generic")]
    Generic(crate::GenericSignIn),
    /// Microsoft.
    #[cfg(feature = "microsoft")]
    Microsoft(crate::MicrosoftSignIn),
    /// Pasted API keys.
    #[cfg(feature = "api_key")]
    ApiKey(crate::ApiKeySignIn),
    /// OpenRouter.
    #[cfg(feature = "openrouter")]
    OpenRouter(crate::OpenRouterSignIn),
}

impl Provider for FamilyProvider {
    type Session = FamilySession;
    type SignIn = FamilySignIn;

    fn spec(&self) -> &ProviderSpec {
        each_family!(self, p => p.spec())
    }

    async fn discover(
        &self,
        account: &AccountId,
        presented: &Presented,
    ) -> Result<Vec<Claim>, ProviderError> {
        each_family!(self, p => p.discover(account, presented).await)
    }

    async fn open(
        &self,
        account: &AccountId,
        presented: Presented,
    ) -> Result<FamilySession, ProviderError> {
        match self {
            #[cfg(feature = "nextcloud")]
            Self::Nextcloud(p) => p
                .open(account, presented)
                .await
                .map(FamilySession::Nextcloud),
            #[cfg(feature = "generic")]
            Self::Generic(p) => p.open(account, presented).await.map(FamilySession::Generic),
            #[cfg(feature = "microsoft")]
            Self::Microsoft(p) => p
                .open(account, presented)
                .await
                .map(FamilySession::Microsoft),
            #[cfg(feature = "api_key")]
            Self::ApiKey(p) => p.open(account, presented).await.map(FamilySession::ApiKey),
            #[cfg(feature = "openrouter")]
            Self::OpenRouter(p) => p
                .open(account, presented)
                .await
                .map(FamilySession::OpenRouter),
            #[allow(unreachable_patterns)]
            _ => {
                let _ = (account, presented);
                unreachable!("no family is built")
            }
        }
    }

    fn sign_in(&self, start: SignInStart) -> Result<FamilySignIn, ProviderError> {
        match self {
            #[cfg(feature = "nextcloud")]
            Self::Nextcloud(p) => p.sign_in(start).map(FamilySignIn::Nextcloud),
            #[cfg(feature = "generic")]
            Self::Generic(p) => p.sign_in(start).map(FamilySignIn::Generic),
            #[cfg(feature = "microsoft")]
            Self::Microsoft(p) => p.sign_in(start).map(FamilySignIn::Microsoft),
            #[cfg(feature = "api_key")]
            Self::ApiKey(p) => p.sign_in(start).map(FamilySignIn::ApiKey),
            #[cfg(feature = "openrouter")]
            Self::OpenRouter(p) => p.sign_in(start).map(FamilySignIn::OpenRouter),
            #[allow(unreachable_patterns)]
            _ => {
                let _ = start;
                unreachable!("no family is built")
            }
        }
    }

    async fn revoke(&self, presented: &Presented) -> Result<RevokeOutcome, ProviderError> {
        each_family!(self, p => p.revoke(presented).await)
    }
}

impl ProviderSession for FamilySession {
    async fn access_token(&self, audience: &Audience) -> Result<IssuedToken, ProviderError> {
        each_family!(self, s => s.access_token(audience).await)
    }

    fn renewed(&self) -> Option<Credential> {
        each_family!(self, s => s.renewed())
    }
}

impl SignIn for FamilySignIn {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        each_family!(self, s => s.next(input).await)
    }
}
