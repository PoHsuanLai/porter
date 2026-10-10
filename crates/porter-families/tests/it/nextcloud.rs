//! The Nextcloud family against the fake Nextcloud: Login Flow v2 with the scripted browser,
//! discovery through OCS and DAV, a refused or abandoned sign-in, revocation.
#![cfg(feature = "nextcloud")]

use crate::common;

use common::{Fakes, all_on, plain};
use porter_core::capability::CapabilityKind;
use porter_core::sheet::{FieldKind, SignInFault, SignInInput};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AuthKind, Credential, EndpointUrl, Family,
    LoginName, Offer, ProviderId, Restriction, SecretPurpose, SecretText, ServiceEndpoint, Tls,
    WebUrl,
};
use porter_fake::FakeServer;
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Scheme, send};
use porter_fake_servers::{FakeNextcloud, LoginPolicy, NextcloudHandle, Running, shipped};
use porter_families::{NextcloudProvider, NextcloudSession};
use porter_http::{NoSleep, SharedHttp};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, RevokeOutcome, SignInMode, SignInStart,
    SignInStep, Signed,
};
use std::time::Duration;

struct World {
    nextcloud: Running<NextcloudHandle>,
    /// The provider file as shipped: it names no server.
    open: NextcloudProvider,
    /// The provider file pointed at the fake, as a deployment's own file may.
    fixed: NextcloudProvider,
}

impl World {
    fn base(&self) -> EndpointUrl {
        EndpointUrl::parse(self.nextcloud.base_url()).expect("base url")
    }
}

fn provider(spec: porter_provider::ProviderSpec) -> NextcloudProvider {
    NextcloudProvider::new(spec, SharedHttp::new(Fakes::loopback()), NoSleep).with_pacing(
        porter_families::Pacing {
            first: Duration::from_secs(1),
            step: Duration::from_secs(10),
            limit: Duration::from_secs(120),
        },
    )
}

async fn world() -> World {
    let fake = FakeNextcloud::bind("alice").await.expect("fake");
    let handle = fake.handle();
    let pointed = fake.rewrite(&shipped::nextcloud());
    World {
        nextcloud: Running::spawn(fake, handle),
        open: provider(shipped::nextcloud()),
        fixed: provider(pointed),
    }
}

/// The account the fake's Nextcloud gives "alice": what the registry would hold after adding.
fn held(world: &World) -> Account {
    let at =
        |path: &str| EndpointUrl::parse(&format!("{}{path}", world.base().as_str())).expect("url");
    Account {
        id: AccountId::parse("nextcloud-alice").expect("id"),
        provider: ProviderId::parse("nextcloud").expect("id"),
        label: AccountLabel("alice".into()),
        state: AccountState::Ok,
        auth: AuthKind::LoginFlowV2,
        capabilities: vec![],
        restriction: Restriction::none(),
        endpoints: vec![ServiceEndpoint {
            family: Family::WebDav,
            url: at("/remote.php/dav/files/alice/"),
            tls: Tls::Plain,
            login: LoginName("alice".into()),
        }],
    }
}

fn add() -> SignInStart {
    SignInStart::new(SignInMode::Add)
}

/// The person opens the page the sign-in sent them to.
async fn visit(url: &WebUrl) {
    let (address, target) = split_loopback(url.as_str()).expect("loopback page");
    let page = send(&address, Scheme::Http, &Request::new("GET", &target))
        .await
        .expect("page");
    assert_eq!(page.status, 200);
}

/// Opens the page when the sign-in asks, confirms the review, answers a server question.
fn person(server: String) -> impl FnMut(&SignInStep) -> (SignInInput, Option<WebUrl>) {
    move |step| match step {
        SignInStep::AskFields(_) => (
            SignInInput::Fields(vec![plain(FieldKind::Server, &server)]),
            None,
        ),
        SignInStep::OpenBrowser { url } => (SignInInput::Poll, Some(url.clone())),
        SignInStep::Waiting => (SignInInput::Poll, None),
        SignInStep::Review { claims, .. } => (
            SignInInput::Confirm(all_on(claims.iter().map(|c| c.offer.kind()))),
            None,
        ),
        _ => (SignInInput::Cancel, None),
    }
}

/// Drives a sign-in with the person above, who opens the page when sent there.
async fn sign_in(
    provider: &NextcloudProvider,
    start: SignInStart,
    server: &str,
) -> Vec<SignInStep> {
    let mut signin = provider.sign_in(start).expect("sign-in");
    let mut say = person(server.to_owned());
    let mut steps = Vec::new();
    let mut input = SignInInput::Start;
    for _ in 0..200 {
        let step = porter_provider::SignIn::next(&mut signin, input).await;
        let (next, page) = say(&step);
        if let Some(url) = page {
            visit(&url).await;
        }
        let over = matches!(step, SignInStep::Done(_) | SignInStep::Failed(_));
        steps.push(step);
        input = next;
        if over {
            return steps;
        }
    }
    panic!("no end: {steps:#?}");
}

fn signed(steps: &[SignInStep]) -> &Signed {
    match steps.last() {
        Some(SignInStep::Done(signed)) => signed,
        other => panic!("expected Done, got {other:?}"),
    }
}

#[tokio::test]
async fn login_flow_v2_signs_in_and_reviews_what_the_server_offers() {
    let world = world().await;
    world.nextcloud.set_login_policy(LoginPolicy::Approve);
    let steps = sign_in(&world.fixed, add(), "").await;

    // The page, then waiting (the first poll comes before the person is done), then a review.
    let kinds: Vec<&str> = steps
        .iter()
        .map(|s| match s {
            SignInStep::OpenBrowser { .. } => "browser",
            SignInStep::Waiting => "waiting",
            SignInStep::Review { .. } => "review",
            SignInStep::Done(_) => "done",
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(kinds, ["browser", "review", "done"]);
    let SignInStep::OpenBrowser { url } = &steps[0] else {
        unreachable!()
    };
    assert!(url.as_str().contains("/index.php/login/v2/flow/"), "{url}");

    let signed = signed(&steps);
    let host = format!("alice@{}", world.base().origin().host);
    assert!(signed.label.0.starts_with(&host), "{:?}", signed.label);
    let present: Vec<CapabilityKind> = signed
        .claims
        .iter()
        .filter(|c| matches!(c.offer, Offer::Present(_)))
        .map(|c| c.offer.kind())
        .collect();
    assert_eq!(
        present,
        [
            CapabilityKind::Storage,
            CapabilityKind::Calendar,
            CapabilityKind::Contacts,
            CapabilityKind::Notes
        ]
    );
    let tasks = signed
        .claims
        .iter()
        .find(|c| c.offer.kind() == CapabilityKind::Tasks)
        .expect("a tasks claim");
    assert!(
        matches!(tasks.offer, Offer::Absent { .. }),
        "no tasks app on the fake: {tasks:?}"
    );

    let base = world.base();
    let at = |path: &str| format!("{}{path}", base.as_str());
    let endpoints: Vec<(Family, String, Tls, &str)> = signed
        .endpoints
        .iter()
        .map(|e| (e.family, e.url.to_string(), e.tls, e.login.0.as_str()))
        .collect();
    assert_eq!(
        endpoints,
        vec![
            (
                Family::WebDav,
                at("/remote.php/dav/files/alice/"),
                Tls::Plain,
                "alice"
            ),
            (
                Family::CalDav,
                at("/remote.php/dav/calendars/alice/"),
                Tls::Plain,
                "alice"
            ),
            (
                Family::CardDav,
                at("/remote.php/dav/addressbooks/users/alice/"),
                Tls::Plain,
                "alice"
            ),
            (
                Family::NextcloudNotes,
                at("/index.php/apps/notes/api/v1/"),
                Tls::Plain,
                "alice"
            ),
        ]
    );
    for endpoint in &signed.endpoints {
        assert_eq!(endpoint.check(), Ok(()), "{endpoint:?}");
    }

    // The app password the server minted is the credential, and works at the server.
    let [(SecretPurpose::Password, Credential::Password(password))] = signed.credentials.as_slice()
    else {
        panic!("one password: {:?}", signed.credentials);
    };
    assert!(
        world
            .nextcloud
            .app_passwords()
            .contains(&password.expose().to_owned())
    );
    assert_eq!(signed.restriction, porter_core::Restriction::none());
}

#[tokio::test]
async fn a_provider_file_that_names_no_server_asks_for_it_first() {
    let world = world().await;
    world.nextcloud.set_login_policy(LoginPolicy::Approve);
    let typed = world.nextcloud.base_url().to_owned();
    let steps = sign_in(&world.open, add(), &typed).await;
    let SignInStep::AskFields(fields) = &steps[0] else {
        panic!("expected a question first: {steps:?}");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].kind, FieldKind::Server);
    assert!(matches!(steps[1], SignInStep::OpenBrowser { .. }));
    assert!(matches!(steps.last(), Some(SignInStep::Done(_))));
}

#[tokio::test]
async fn a_server_that_is_not_one_ends_the_sign_in_unread() {
    let world = world().await;
    for typed in ["", "http://cloud.example.org", "imaps://mail.example.org"] {
        let steps = sign_in(&world.open, add(), typed).await;
        assert!(
            matches!(
                steps.last(),
                Some(SignInStep::Failed(SignInFault::Unreadable))
            ),
            "{typed:?}: {steps:?}"
        );
    }
    // A server that answers, but is no Nextcloud: the fake autoconfig has no Login Flow v2.
    let elsewhere = porter_fake_servers::FakeAutoconfig::start()
        .await
        .expect("fake");
    let steps = sign_in(&world.open, add(), elsewhere.base_url()).await;
    assert!(matches!(
        steps.last(),
        Some(SignInStep::Failed(SignInFault::Unreadable))
    ));
    // And one that is not there at all.
    let gone = {
        let held = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind");
        format!(
            "http://127.0.0.1:{}",
            held.local_addr().expect("addr").port()
        )
    };
    let steps = sign_in(&world.open, add(), &gone).await;
    assert!(matches!(
        steps.last(),
        Some(SignInStep::Failed(SignInFault::Unreachable))
    ));
}

#[tokio::test]
async fn a_login_nobody_approves_times_out_after_the_pacing_limit_and_mints_nothing() {
    let world = world().await;
    world.nextcloud.set_login_policy(LoginPolicy::Pending);
    let mut signin = world.fixed.sign_in(add()).expect("sign-in");
    let mut polls = 0;
    let mut step = porter_provider::SignIn::next(&mut signin, SignInInput::Start).await;
    assert!(matches!(step, SignInStep::OpenBrowser { .. }));
    while !matches!(step, SignInStep::Failed(_)) {
        step = porter_provider::SignIn::next(&mut signin, SignInInput::Poll).await;
        polls += 1;
        assert!(polls < 1000, "never gave up");
    }
    assert_eq!(step, SignInStep::Failed(SignInFault::TimedOut));
    // Waits of 1, 2, ... 10 seconds, then 10 each: 125 seconds are spent by the 17th poll, so
    // the 18th turn gives up without asking.
    assert_eq!(polls, 18);
    assert_eq!(world.nextcloud.app_passwords(), Vec::<String>::new());
    let poll_hits = world
        .nextcloud
        .hits()
        .into_iter()
        .filter(|h| h.target == "/index.php/login/v2/poll")
        .count();
    assert_eq!(poll_hits, 17);
}

#[tokio::test]
async fn cancelling_ends_the_sign_in_and_a_new_start_begins_a_new_flow() {
    let world = world().await;
    let mut signin = world.fixed.sign_in(add()).expect("sign-in");
    let first = porter_provider::SignIn::next(&mut signin, SignInInput::Start).await;
    let again = porter_provider::SignIn::next(&mut signin, SignInInput::Start).await;
    let (SignInStep::OpenBrowser { url: a }, SignInStep::OpenBrowser { url: b }) = (&first, &again)
    else {
        panic!("two pages: {first:?} {again:?}");
    };
    assert_ne!(a, b, "a fresh flow each time");
    assert_eq!(
        porter_provider::SignIn::next(&mut signin, SignInInput::Cancel).await,
        SignInStep::Failed(SignInFault::Cancelled)
    );
    // Nothing is left to poll.
    assert!(matches!(
        porter_provider::SignIn::next(&mut signin, SignInInput::Poll).await,
        SignInStep::Failed(_)
    ));
}

#[tokio::test]
async fn signing_in_again_replaces_the_password_without_a_review() {
    let world = world().await;
    world.nextcloud.set_login_policy(LoginPolicy::Approve);
    let start = SignInStart::new(SignInMode::Reauthenticate {
        account: AccountId::parse("nextcloud-alice").expect("id"),
        endpoints: held(&world).endpoints,
    });
    let steps = sign_in(&world.fixed, start, "").await;
    assert!(
        !steps.iter().any(|s| matches!(s, SignInStep::Review { .. })),
        "{steps:?}"
    );
    let signed = signed(&steps);
    assert_eq!(signed.credentials.len(), 1);
    assert_eq!(signed.endpoints.len(), 4);
}

#[tokio::test]
async fn signing_in_again_starts_at_the_accounts_own_server_without_asking_for_it() {
    let world = world().await;
    world.nextcloud.set_login_policy(LoginPolicy::Approve);
    // The shipped file names no server, so an Add asks; a sign-in again reads it off the
    // account's endpoints.
    let start = SignInStart::new(SignInMode::Reauthenticate {
        account: AccountId::parse("nextcloud-alice").expect("id"),
        endpoints: held(&world).endpoints,
    });
    let steps = sign_in(&world.open, start, "").await;
    assert!(
        !steps
            .iter()
            .any(|s| matches!(s, SignInStep::AskFields(_) | SignInStep::Review { .. })),
        "{steps:?}"
    );
    assert!(
        matches!(steps[0], SignInStep::OpenBrowser { .. }),
        "{steps:?}"
    );
    assert_eq!(signed(&steps).endpoints.len(), 4);

    // An account with no Nextcloud endpoint to read a server from asks like an Add.
    let start = SignInStart::new(SignInMode::Reauthenticate {
        account: AccountId::parse("nextcloud-alice").expect("id"),
        endpoints: vec![],
    });
    let typed = world.nextcloud.base_url().to_owned();
    let steps = sign_in(&world.open, start, &typed).await;
    assert!(matches!(steps[0], SignInStep::AskFields(_)), "{steps:?}");
}

#[tokio::test]
async fn discovery_with_a_password_the_server_refuses_is_unauthorized() {
    let world = world().await;
    world.nextcloud.seed_app_password("good");
    let account = held(&world);
    let claims = world
        .open
        .discover(&account, &password("good"))
        .await
        .expect("discovered");
    assert_eq!(
        claims
            .iter()
            .filter(|c| matches!(c.offer, Offer::Present(_)))
            .count(),
        4
    );
    assert_eq!(
        world.open.discover(&account, &password("bad")).await,
        Err(ProviderError::Unauthorized)
    );
    // No credential, an account with no server to ask, and a credential of the wrong kind.
    assert_eq!(
        world.open.discover(&account, &Presented::Anonymous).await,
        Err(ProviderError::Unauthorized)
    );
    let homeless = Account {
        endpoints: vec![],
        ..account
    };
    assert_eq!(
        world.open.discover(&homeless, &password("good")).await,
        Err(ProviderError::Unreadable)
    );
}

fn password(text: &str) -> Presented {
    Presented::Credential(Credential::Password(SecretText::new(text)))
}

#[tokio::test]
async fn revoking_deletes_the_app_password_at_the_server() {
    let world = world().await;
    world.nextcloud.seed_app_password("mine");
    world.nextcloud.seed_app_password("another device");
    let account = held(&world);
    assert_eq!(
        world.open.revoke(&account, &password("mine")).await,
        Ok(RevokeOutcome::Revoked)
    );
    assert_eq!(
        world.nextcloud.app_passwords(),
        vec!["another device".to_owned()]
    );
    // Already gone: the server refuses the password, which is what revoking was for.
    assert_eq!(
        world.open.revoke(&account, &password("mine")).await,
        Ok(RevokeOutcome::Revoked)
    );
    assert_eq!(
        world.nextcloud.app_passwords(),
        vec!["another device".to_owned()]
    );
    let homeless = Account {
        endpoints: vec![],
        ..account
    };
    assert_eq!(
        world.open.revoke(&homeless, &password("mine")).await,
        Err(ProviderError::Unreadable)
    );
}

#[tokio::test]
async fn a_session_holds_the_account_and_never_hands_out_its_password() {
    let world = world().await;
    let account = AccountId::parse("nextcloud-alice").expect("id");
    let presented = Presented::Credential(Credential::Password(SecretText::new("pw")));
    let session: NextcloudSession = world.open.open(&account, presented).await.expect("session");
    assert_eq!(session.account(), &account);
    assert_eq!(session.renewed(), None);
    assert_eq!(
        session
            .access_token(&porter_core::Audience("webdav".into()))
            .await
            .err(),
        Some(ProviderError::Forbidden)
    );
    assert_eq!(
        world.open.open(&account, Presented::Anonymous).await.err(),
        Some(ProviderError::Unauthorized)
    );
    let key = Presented::Credential(Credential::ApiKey(SecretText::new("k")));
    assert_eq!(
        world.open.open(&account, key).await.err(),
        Some(ProviderError::Unreadable)
    );
}
