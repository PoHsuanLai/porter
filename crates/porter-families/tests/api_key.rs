//! The ApiKey family against a loopback fake of each company's key check: the key is asked for
//! in a hidden field, presented the way the company documents (the fake refuses a right key in a
//! wrong header), checked with one models-list call whose answer is dropped, and never shown or
//! handed to an app.
#![cfg(feature = "api_key")]

use porter_core::capability::{Capability, CapabilityKind};
use porter_core::sheet::{
    Entry, FieldAnswer, FieldKind, FieldValue, Presence, SignInFault, SignInInput,
};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, Audience, AuthKind, Credential, EndpointUrl,
    Offer, Provenance, Restriction, SecretPurpose, SecretText,
};
use porter_fake_servers::{Auth, Call, FakeLlmApi, LlmApiHandle, Running, shipped};
use porter_families::ApiKeyProvider;
use porter_http::HyperHttp;
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, RevokeOutcome, SignIn, SignInMode,
    SignInStart, SignInStep, Signed,
};
use std::time::Duration;

const PASTED: &str = "sk-live-pasted-key-0001";

/// Each shipped company, how its fake wants the key, and where its keys page is.
const COMPANIES: &[(&str, Auth, &str)] = &[
    (
        "anthropic",
        Auth::XApiKey,
        "https://console.anthropic.com/settings/keys",
    ),
    (
        "google-ai",
        Auth::XGoogApiKey,
        "https://aistudio.google.com/apikey",
    ),
    (
        "moonshot",
        Auth::Bearer,
        "https://platform.moonshot.ai/console/api-keys",
    ),
    (
        "openai",
        Auth::Bearer,
        "https://platform.openai.com/api-keys",
    ),
    (
        "openrouter",
        Auth::BearerKey,
        "https://openrouter.ai/settings/keys",
    ),
];

fn account(provider: &str) -> Account {
    Account {
        id: AccountId::parse("cloud-1").expect("id"),
        provider: porter_core::ProviderId::parse(provider).expect("provider id"),
        label: AccountLabel("cloud".into()),
        state: AccountState::Ok,
        auth: AuthKind::ApiKey,
        capabilities: Vec::new(),
        restriction: Restriction::none(),
        endpoints: Vec::new(),
    }
}

fn start(mode: SignInMode) -> SignInStart {
    SignInStart { mode }
}

fn reauth() -> SignInMode {
    SignInMode::Reauthenticate {
        account: AccountId::parse("cloud-1").expect("id"),
        endpoints: Vec::new(),
    }
}

async fn company(id: &str, auth: Auth) -> (Running<LlmApiHandle>, ApiKeyProvider) {
    let fake = FakeLlmApi::start(auth).await.expect("fake company");
    let provider = ApiKeyProvider::with_http(fake.point(&shipped::ai(id)), HyperHttp::default());
    (fake, provider)
}

fn typed(text: &str) -> SignInInput {
    SignInInput::Fields(vec![FieldAnswer {
        kind: FieldKind::ApiKey,
        value: FieldValue::Secret(SecretText::new(text)),
    }])
}

fn live(key: &str) -> Presented {
    Presented::Credential(Credential::ApiKey(SecretText::new(key)))
}

fn signed_of(step: SignInStep) -> Signed {
    match step {
        SignInStep::Done(signed) => signed,
        other => panic!("not done: {other:?}"),
    }
}

fn key_of(signed: &Signed) -> String {
    match signed.credentials.as_slice() {
        [(SecretPurpose::ApiKey, Credential::ApiKey(key))] => key.expose().to_owned(),
        other => panic!("one api key expected: {other:?}"),
    }
}

fn assert_one_llm_claim(signed: &Signed) {
    // The one claim about the account itself; the agent programs a file names are claims about
    // those programs (`Subject::Agent`).
    let own: Vec<_> = signed
        .claims
        .iter()
        .filter(|claim| claim.subject == porter_core::Subject::Account)
        .collect();
    assert_eq!(own.len(), 1);
    let claim = own[0];
    assert_eq!(claim.provenance, Provenance::Probed);
    assert!(matches!(&claim.offer, Offer::Present(Capability::Llm(_))));
    assert_eq!(claim.offer.kind(), CapabilityKind::Llm);
    assert!(signed.endpoints.is_empty());
}

/// The one check a key got, and that it was presented as `auth` wants and in no other header.
fn assert_presented(call: &Call, auth: Auth, key: &str) {
    assert_eq!(call.status, 200, "{call:?}");
    let bearer = format!("Bearer {key}");
    let want = match auth {
        Auth::XApiKey => (Some(key), Some("2023-06-01"), None, None),
        Auth::XGoogApiKey => (None, None, Some(key), None),
        Auth::Bearer | Auth::BearerKey => (None, None, None, Some(bearer.as_str())),
    };
    let got = (
        call.x_api_key.as_deref(),
        call.anthropic_version.as_deref(),
        call.x_goog_api_key.as_deref(),
        call.authorization.as_deref(),
    );
    assert_eq!(got, want, "{call:?}");
    assert!(
        !call.target.contains(key),
        "the key is not in the url: {call:?}"
    );
    let leaf = if auth == Auth::BearerKey {
        "/key"
    } else {
        "/models"
    };
    assert!(call.target.ends_with(leaf), "{call:?}");
}

#[tokio::test]
async fn a_pasted_key_is_asked_for_hidden_checked_the_documented_way_and_reviewed() {
    for (id, auth, _) in COMPANIES {
        let (fake, provider) = company(id, *auth).await;
        fake.seed_key(PASTED);
        let mut signin = provider.sign_in(start(SignInMode::Add)).expect("sign-in");
        let SignInStep::AskFields(fields) = signin.next(SignInInput::Start).await else {
            panic!("{id}: a form");
        };
        assert_eq!(fields.len(), 1, "{id}");
        assert_eq!(
            (fields[0].kind, fields[0].entry, fields[0].presence),
            (FieldKind::ApiKey, Entry::Secret, Presence::Required),
            "{id}"
        );
        // A paste brings spaces along.
        let review = signin.next(typed(&format!("  {PASTED}\n"))).await;
        let SignInStep::Review { label, claims, .. } = &review else {
            panic!("{id}: a review: {review:?}");
        };
        assert_eq!(label.0, shipped::ai(id).label, "{id}");
        assert_eq!(claims.len(), 1, "{id}");
        let signed = signed_of(signin.next(SignInInput::Confirm(Vec::new())).await);
        assert_eq!(key_of(&signed), PASTED, "{id}");
        assert_one_llm_claim(&signed);
        let calls = fake.calls();
        assert_eq!(calls.len(), 1, "{id}: one cheap call: {calls:?}");
        assert_presented(&calls[0], *auth, PASTED);
        let shown = format!("{signed:?} {review:?}");
        assert!(!shown.contains(PASTED), "{id}: {shown}");
    }
}

#[tokio::test]
async fn a_key_in_the_wrong_header_is_refused_by_the_company() {
    // The family reads which header from the file's wire: Anthropic's file pointed at a fake that
    // wants a bearer token is a key the company would refuse.
    let fake = FakeLlmApi::start(Auth::Bearer).await.expect("fake");
    fake.seed_key(PASTED);
    let mismatched = porter_fake_servers::shipped::point(
        &shipped::ai("anthropic"),
        porter_core::Family::Messages,
        &fake.api_url(),
    );
    let provider = ApiKeyProvider::with_http(mismatched, HyperHttp::default());
    assert_eq!(
        provider
            .discover(&account("anthropic"), &live(PASTED))
            .await,
        Err(ProviderError::Unauthorized)
    );
}

#[tokio::test]
async fn a_pasted_key_that_fails_ends_the_sign_in_with_the_reason() {
    let (fake, provider) = company("anthropic", Auth::XApiKey).await;
    fake.seed_key(PASTED);
    let cases = [
        (
            "a key the company refuses",
            typed("sk-wrong"),
            SignInFault::Refused,
        ),
        ("an empty paste", typed("   "), SignInFault::Unreadable),
        (
            "text that is not a secret",
            SignInInput::Fields(vec![FieldAnswer {
                kind: FieldKind::ApiKey,
                value: FieldValue::Plain(PASTED.into()),
            }]),
            SignInFault::Unreadable,
        ),
        (
            "no answers",
            SignInInput::Fields(Vec::new()),
            SignInFault::Unreadable,
        ),
        ("a cancel", SignInInput::Cancel, SignInFault::Cancelled),
        ("out of turn", SignInInput::Poll, SignInFault::Unreadable),
    ];
    for (name, input, want) in cases {
        let mut signin = provider.sign_in(start(SignInMode::Add)).expect("sign-in");
        signin.next(SignInInput::Start).await;
        assert_eq!(signin.next(input).await, SignInStep::Failed(want), "{name}");
    }
}

#[tokio::test]
async fn signing_in_again_skips_the_review_and_a_dead_server_is_unreachable() {
    let (fake, provider) = company("openai", Auth::Bearer).await;
    fake.seed_key(PASTED);
    let mut signin = provider.sign_in(start(reauth())).expect("sign-in");
    signin.next(SignInInput::Start).await;
    let signed = signed_of(signin.next(typed(PASTED)).await);
    assert_eq!(key_of(&signed), PASTED);

    // A fresh client: the first one may still hold a kept-alive connection to a task of the fake.
    let spec = fake.point(&shipped::ai("openai"));
    drop(fake);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let provider = ApiKeyProvider::with_http(spec, HyperHttp::default());
    let mut signin = provider.sign_in(start(SignInMode::Add)).expect("sign-in");
    signin.next(SignInInput::Start).await;
    assert_eq!(
        signin.next(typed(PASTED)).await,
        SignInStep::Failed(SignInFault::Unreachable)
    );
}

#[tokio::test]
async fn discover_checks_the_key_and_a_dead_key_is_unauthorized() {
    for (id, auth, _) in COMPANIES {
        let (fake, provider) = company(id, *auth).await;
        let holder = account(id);
        fake.seed_key("sk-seeded");
        let claims = provider
            .discover(&holder, &live("sk-seeded"))
            .await
            .expect("live");
        assert_eq!(claims.len(), 1, "{id}");
        assert_eq!(claims[0].provenance, Provenance::Probed);
        assert_presented(&fake.calls()[0], *auth, "sk-seeded");

        fake.delete_key("sk-seeded");
        assert_eq!(
            provider.discover(&holder, &live("sk-seeded")).await,
            Err(ProviderError::Unauthorized),
            "{id}"
        );
        for presented in [
            Presented::Anonymous,
            Presented::Credential(Credential::Password(SecretText::new("pw"))),
        ] {
            assert_eq!(
                provider.discover(&holder, &presented).await,
                Err(ProviderError::Unauthorized),
                "{id}"
            );
            assert!(provider.open(&holder.id, presented).await.is_err());
        }
    }
}

#[tokio::test]
async fn a_session_never_hands_a_key_to_an_app_and_revoke_names_the_keys_page() {
    for (id, auth, page) in COMPANIES {
        let (_fake, provider) = company(id, *auth).await;
        let holder = account(id);
        let session = provider.open(&holder.id, live("sk-x")).await.expect("open");
        for audience in ["chat_completions", "messages", "anything"] {
            assert_eq!(
                session.access_token(&Audience(audience.into())).await,
                Err(ProviderError::Forbidden),
                "{id} {audience}"
            );
        }
        assert_eq!(session.renewed(), None);
        assert_eq!(
            provider.revoke(&holder, &live("sk-x")).await,
            Ok(RevokeOutcome::Manual(
                EndpointUrl::parse(page).expect("url")
            )),
            "{id}"
        );
    }
}

#[tokio::test]
async fn a_file_with_no_known_keys_page_has_nothing_to_revoke_with() {
    let mut spec = shipped::ai("openai");
    spec.id = porter_core::ProviderId::parse("someone-else").expect("id");
    let provider = ApiKeyProvider::with_http(spec, HyperHttp::default());
    assert_eq!(
        provider
            .revoke(&account("someone-else"), &live("sk-x"))
            .await,
        Ok(RevokeOutcome::Unsupported)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_account_holds_a_claim_for_each_agent_program_its_file_names() {
    use porter_core::{Provenance, Subject};
    // (company, the programs its file names)
    const NAMED: &[(&str, &[&str])] = &[
        ("anthropic", &["claude-code"]),
        ("google-ai", &["gemini-cli"]),
        ("openai", &["codex"]),
        ("moonshot", &[]),
        ("openrouter", &[]),
    ];
    for (id, auth, _) in COMPANIES {
        let (fake, provider) = company(id, *auth).await;
        fake.seed_key(PASTED);
        let claims = provider
            .discover(&account(id), &live(PASTED))
            .await
            .expect("a live key");
        let programs: Vec<&str> = claims
            .iter()
            .filter_map(|claim| match &claim.subject {
                Subject::Agent(program) => Some(program.as_str()),
                _ => None,
            })
            .collect();
        let want = NAMED
            .iter()
            .find(|(c, _)| c == id)
            .map(|(_, p)| *p)
            .expect("row");
        assert_eq!(programs, want, "{id}");
        for claim in claims
            .iter()
            .filter(|c| matches!(c.subject, Subject::Agent(_)))
        {
            // Declared by the file, and the capability is about the program its subject names.
            assert_eq!(claim.provenance, Provenance::Declared);
            let Offer::Present(Capability::Agent(agent)) = &claim.offer else {
                panic!("{id}: not an agent offer: {claim:?}");
            };
            assert_eq!(claim.subject, Subject::Agent(agent.program.clone()));
        }
    }
}
