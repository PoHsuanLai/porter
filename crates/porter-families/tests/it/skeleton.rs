//! A family that is not built refuses in plain words and is never ready: nothing that reaches
//! it by mistake panics, and the add list never shows it.
#![cfg(feature = "openrouter")]

use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AuthKind, ProviderId, Restriction,
};
use porter_families::OpenRouterProvider;
use porter_provider::{
    Presented, Provider, ProviderError, Readiness, RevokeOutcome, SignInMode, SignInStart,
    parse_provider,
};

fn provider() -> OpenRouterProvider {
    let text = porter_provider::SHIPPED_FILES
        .iter()
        .find(|(file, _)| *file == "openrouter")
        .map(|(_, text)| *text)
        .expect("openrouter ships");
    OpenRouterProvider::new(parse_provider(text).expect("parses"))
}

fn account() -> Account {
    Account {
        id: AccountId::parse("openrouter").expect("id"),
        provider: ProviderId::parse("openrouter").expect("provider id"),
        label: AccountLabel("OpenRouter".into()),
        state: AccountState::Ok,
        auth: AuthKind::OAuthMintsKey,
        capabilities: Vec::new(),
        restriction: Restriction::none(),
        endpoints: Vec::new(),
    }
}

#[tokio::test]
async fn every_method_of_an_unbuilt_family_refuses_and_none_panics() {
    let provider = provider();
    let refused = Some(ProviderError::Forbidden);
    assert_eq!(
        provider
            .discover(&account(), &Presented::Anonymous)
            .await
            .err(),
        refused
    );
    assert_eq!(
        provider
            .open(&account().id, Presented::Anonymous)
            .await
            .err(),
        refused
    );
    let add = SignInStart {
        mode: SignInMode::Add,
    };
    assert_eq!(provider.sign_in(add).err(), refused);
    assert_eq!(
        provider
            .revoke(&account(), &Presented::Anonymous)
            .await
            .expect("revoke answers"),
        RevokeOutcome::Unsupported
    );
}

#[test]
fn an_unbuilt_family_is_never_ready_so_the_add_list_never_shows_it() {
    assert_ne!(provider().readiness(), Readiness::Ready);
}
