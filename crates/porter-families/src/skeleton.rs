//! One family's frozen shape: its provider, session and sign-in types over the `Provider`
//! seam, every body waiting for the lane that builds the family.

/// Declares `$provider`, `$session` and `$signin` for one family, each method `todo!()` naming
/// `$what`.
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
                _account: &porter_core::AccountId,
                _presented: &porter_provider::Presented,
            ) -> Result<Vec<porter_core::Claim>, porter_provider::ProviderError> {
                todo!(concat!("discover what a ", $what, " account can do"))
            }

            async fn open(
                &self,
                account: &porter_core::AccountId,
                _presented: porter_provider::Presented,
            ) -> Result<$session, porter_provider::ProviderError> {
                let _ = account;
                todo!(concat!("open a ", $what, " account from its credential"))
            }

            fn sign_in(
                &self,
                start: porter_provider::SignInStart,
            ) -> Result<$signin, porter_provider::ProviderError> {
                let _ = start;
                todo!(concat!("begin the ", $what, " sign-in"))
            }

            async fn revoke(
                &self,
                _presented: &porter_provider::Presented,
            ) -> Result<porter_provider::RevokeOutcome, porter_provider::ProviderError> {
                todo!(concat!("revoke a ", $what, " account at its provider"))
            }
        }

        impl porter_provider::ProviderSession for $session {
            async fn access_token(
                &self,
                _audience: &porter_core::Audience,
            ) -> Result<porter_core::IssuedToken, porter_provider::ProviderError> {
                let _ = &self.account;
                todo!(concat!("a short-lived token for a ", $what, " account"))
            }

            fn renewed(&self) -> Option<porter_core::Credential> {
                todo!(concat!(
                    "the ",
                    $what,
                    " credential, when renewal changed it"
                ))
            }
        }

        impl porter_provider::SignIn for $signin {
            async fn next(
                &mut self,
                _input: porter_core::sheet::SignInInput,
            ) -> porter_provider::SignInStep {
                let _ = &self.start;
                todo!(concat!("the ", $what, " sign-in steps"))
            }
        }
    };
}
pub(crate) use family_skeleton;
