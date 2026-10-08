//! One family's frozen shape: its provider, session and sign-in types over the `Provider`
//! seam, every body waiting for the lane that builds the family. A body that is not built
//! refuses (`Forbidden`, `Failed(Forbidden)`, `Unsupported`) and the provider is never ready, so
//! nothing that reaches it by mistake panics and the add list never shows it.

/// Declares `$provider`, `$session` and `$signin` for one family, each method refusing in the
/// plain way: the family named `$what` is not built.
macro_rules! family_skeleton {
    ($provider:ident, $session:ident, $signin:ident, $what:literal) => {
        #[doc = concat!("The ", $what, " family's provider.")]
        #[derive(Debug, Clone)]
        pub struct $provider {
            spec: porter_provider::ProviderSpec,
        }

        impl $provider {
            /// The provider serving the accounts of `spec`.
            pub fn new(spec: porter_provider::ProviderSpec) -> Self {
                Self { spec }
            }
        }

        #[doc = concat!("An open ", $what, " account.")]
        #[derive(Debug)]
        pub struct $session {
            account: porter_core::AccountId,
        }

        #[doc = concat!("The ", $what, " sign-in conversation.")]
        #[derive(Debug)]
        pub struct $signin {
            start: porter_provider::SignInStart,
        }

        impl porter_provider::Provider for $provider {
            type Session = $session;
            type SignIn = $signin;

            fn spec(&self) -> &porter_provider::ProviderSpec {
                &self.spec
            }

            async fn discover(
                &self,
                _account: &porter_core::Account,
                _presented: &porter_provider::Presented,
            ) -> Result<Vec<porter_core::Claim>, porter_provider::ProviderError> {
                Err(porter_provider::ProviderError::Forbidden)
            }

            async fn open(
                &self,
                account: &porter_core::AccountId,
                _presented: porter_provider::Presented,
            ) -> Result<$session, porter_provider::ProviderError> {
                let _ = account;
                Err(porter_provider::ProviderError::Forbidden)
            }

            fn sign_in(
                &self,
                start: porter_provider::SignInStart,
            ) -> Result<$signin, porter_provider::ProviderError> {
                let _ = start;
                Err(porter_provider::ProviderError::Forbidden)
            }

            /// Never ready: a family that is not built has nothing to sign in to, so the add
            /// list does not show it.
            fn readiness(&self) -> porter_provider::Readiness {
                porter_provider::Readiness::NeedsClient
            }

            async fn revoke(
                &self,
                _account: &porter_core::Account,
                _presented: &porter_provider::Presented,
            ) -> Result<porter_provider::RevokeOutcome, porter_provider::ProviderError> {
                Ok(porter_provider::RevokeOutcome::Unsupported)
            }
        }

        impl porter_provider::ProviderSession for $session {
            async fn access_token(
                &self,
                _audience: &porter_core::Audience,
            ) -> Result<porter_core::IssuedToken, porter_provider::ProviderError> {
                let _ = &self.account;
                Err(porter_provider::ProviderError::Forbidden)
            }

            fn renewed(&self) -> Option<porter_core::Credential> {
                None
            }
        }

        impl porter_provider::SignIn for $signin {
            async fn next(
                &mut self,
                _input: porter_core::sheet::SignInInput,
            ) -> porter_provider::SignInStep {
                let _ = &self.start;
                porter_provider::SignInStep::Failed(porter_core::sheet::SignInFault::Forbidden)
            }
        }
    };
}
pub(crate) use family_skeleton;
