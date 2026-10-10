//! The OpenRouter family (lane AI2 fills it).

use crate::skeleton::family_skeleton;

family_skeleton!(
    OpenRouterProvider,
    OpenRouterSession,
    OpenRouterSignIn,
    "OpenRouter"
);

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::sheet::{SignInFault, SignInInput};
    use porter_core::{AccountId, Audience};
    use porter_provider::{
        ProviderError, ProviderSession, SignIn, SignInMode, SignInStart, SignInStep,
    };

    #[tokio::test]
    async fn the_unbuilt_session_and_sign_in_refuse_instead_of_panicking() {
        let session = OpenRouterSession {
            account: AccountId::parse("openrouter").expect("id"),
        };
        assert_eq!(
            session.access_token(&Audience("x".into())).await.err(),
            Some(ProviderError::Forbidden)
        );
        assert_eq!(session.renewed(), None);

        let mut sign_in = OpenRouterSignIn {
            start: SignInStart::new(SignInMode::Add),
        };
        assert_eq!(
            sign_in.next(SignInInput::Start).await,
            SignInStep::Failed(SignInFault::Forbidden)
        );
    }
}
