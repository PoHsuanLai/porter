//! The AgentLogin family: adding an agent asks nothing and keeps no credential, a session
//! mints no token, and revoking has nothing to do at a provider.
#![cfg(feature = "agent_login")]

use porter_core::capability::{Capability, CapabilityKind};
use porter_core::sheet::{SignInFault, SignInInput};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, Audience, AuthKind, Credential, Offer,
    Provenance, Restriction, SecretText, Subject,
};
use porter_families::AgentLoginProvider;
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, RevokeOutcome, SignIn, SignInMode,
    SignInStart, SignInStep, parse_provider,
};

fn provider(id: &str) -> AgentLoginProvider {
    let text = porter_provider::SHIPPED_FILES
        .iter()
        .find(|(file, _)| *file == id)
        .map(|(_, text)| *text)
        .unwrap_or_else(|| panic!("{id} ships"));
    AgentLoginProvider::new(parse_provider(text).expect("parses"))
}

fn account(id: &str) -> Account {
    Account {
        id: AccountId::parse(id).expect("id"),
        provider: porter_core::ProviderId::parse(id).expect("provider id"),
        label: AccountLabel("agent".into()),
        state: AccountState::NeedsLogin,
        auth: AuthKind::AgentLogin,
        capabilities: Vec::new(),
        restriction: Restriction::none(),
        endpoints: Vec::new(),
    }
}

fn add() -> SignInStart {
    SignInStart {
        mode: SignInMode::Add,
    }
}

const AGENTS: &[&str] = &["claude-code", "gemini-cli", "codex", "acp-agent"];

#[tokio::test]
async fn adding_is_a_review_then_done_with_no_credential_whatever_the_agent() {
    for id in AGENTS {
        let mut sign_in = provider(id).sign_in(add()).expect("starts");
        // Nothing is asked: the first step is the review of what the agent is.
        let SignInStep::Review {
            claims,
            endpoints,
            label,
            ..
        } = sign_in.next(SignInInput::Start).await
        else {
            panic!("{id}: a review first");
        };
        assert!(endpoints.is_empty(), "{id}");
        assert_eq!(claims.len(), 1, "{id}");
        assert_eq!(claims[0].offer.kind(), CapabilityKind::Agent, "{id}");
        assert_eq!(claims[0].provenance, Provenance::Declared, "{id}");
        assert!(label.0.len() > 2, "{id}");
        let SignInStep::Done(signed) = sign_in.next(SignInInput::Confirm(Vec::new())).await else {
            panic!("{id}: done after the confirm");
        };
        assert!(
            signed.credentials.is_empty(),
            "{id}: no credential of any kind"
        );
        assert!(signed.endpoints.is_empty(), "{id}");
        assert_eq!(signed.label, label, "{id}");
        // The claim is about the program the file names.
        let Subject::Agent(program) = &signed.claims[0].subject else {
            panic!("{id}: not about an agent");
        };
        let Offer::Present(Capability::Agent(agent)) = &signed.claims[0].offer else {
            panic!("{id}: not an agent offer");
        };
        assert_eq!(&agent.program, program, "{id}");
    }
}

#[tokio::test]
async fn signing_in_again_is_the_same_two_steps_and_still_holds_nothing() {
    let again = SignInStart {
        mode: SignInMode::Reauthenticate {
            account: AccountId::parse("claude-code").expect("id"),
            endpoints: Vec::new(),
        },
    };
    let mut sign_in = provider("claude-code").sign_in(again).expect("starts");
    assert!(matches!(
        sign_in.next(SignInInput::Start).await,
        SignInStep::Review { .. }
    ));
    let SignInStep::Done(signed) = sign_in.next(SignInInput::Confirm(Vec::new())).await else {
        panic!("done");
    };
    assert!(signed.credentials.is_empty());
}

#[tokio::test]
async fn cancelling_or_answering_out_of_turn_ends_without_an_account() {
    let mut cancelled = provider("codex").sign_in(add()).expect("starts");
    assert_eq!(
        cancelled.next(SignInInput::Cancel).await,
        SignInStep::Failed(SignInFault::Cancelled)
    );
    // A confirm before the review, a typed secret, a poll: none makes an account.
    for input in [
        SignInInput::Confirm(Vec::new()),
        SignInInput::Poll,
        SignInInput::Fields(Vec::new()),
    ] {
        let mut sign_in = provider("codex").sign_in(add()).expect("starts");
        assert_eq!(
            sign_in.next(input).await,
            SignInStep::Failed(SignInFault::Unreadable)
        );
    }
}

#[tokio::test]
async fn discovery_declares_the_programs_and_a_session_holds_and_mints_nothing() {
    for id in AGENTS {
        let provider = provider(id);
        let claims = provider
            .discover(&account(id), &Presented::Anonymous)
            .await
            .expect("claims");
        assert_eq!(claims.len(), 1, "{id}");
        let session = provider
            .open(&AccountId::parse(id).expect("id"), Presented::Anonymous)
            .await
            .expect("opens");
        assert_eq!(
            session
                .access_token(&Audience("acp_agent".into()))
                .await
                .map(|_| ()),
            Err(ProviderError::Forbidden),
            "{id}: the agent's login is the agent's"
        );
        assert_eq!(session.renewed(), None, "{id}");
        assert_eq!(
            provider.revoke(&account(id), &Presented::Anonymous).await,
            Ok(RevokeOutcome::Unsupported),
            "{id}"
        );
    }
}

#[tokio::test]
async fn an_account_that_somehow_presents_a_credential_is_not_an_agent_account() {
    let presented = Presented::Credential(Credential::ApiKey(SecretText::new("sk-not-here")));
    let opened = provider("claude-code")
        .open(&AccountId::parse("claude-code").expect("id"), presented)
        .await;
    assert!(matches!(opened, Err(ProviderError::Unreadable)));
}
