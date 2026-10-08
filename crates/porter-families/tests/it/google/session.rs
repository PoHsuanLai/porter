use super::rig::*;
use porter_core::capability::CapabilityKind as K;
use porter_core::{AccountId, Credential, Offer, SecretText, TokenKind, UnixSeconds};
use porter_fake_servers::IssuerEvent;
use porter_families::ReauthReason;
use porter_provider::{Presented, Provider, ProviderError, ProviderSession, RevokeOutcome};

const SCOPES: &str = "openid https://www.googleapis.com/auth/userinfo.email \
    https://www.googleapis.com/auth/calendar https://www.googleapis.com/auth/tasks \
    https://www.googleapis.com/auth/drive.appdata https://mail.google.com/";

fn account() -> AccountId {
    AccountId::parse("11111111-1111-4111-8111-111111111111").expect("account id")
}

fn held_account() -> porter_core::Account {
    porter_core::Account {
        id: account(),
        provider: porter_core::ProviderId::parse("google").expect("provider"),
        label: porter_core::AccountLabel("ada@gmail.com".into()),
        state: porter_core::AccountState::Ok,
        auth: porter_core::AuthKind::OAuthPkce,
        capabilities: vec![],
        restriction: porter_core::Restriction::none(),
        endpoints: vec![],
    }
}

fn seeded(rig: &Rig) -> (Presented, String) {
    let refresh = rig.google.issuer.seed_refresh(CLIENT_ID, SCOPES);
    (
        Presented::Credential(Credential::OAuth {
            access: SecretText::new("stale"),
            refresh: SecretText::new(refresh.clone()),
            expires_at: UnixSeconds(0),
        }),
        refresh,
    )
}

async fn open(rig: &Rig) -> porter_families::GoogleSession<Wire> {
    rig.provider
        .open(&account(), seeded(rig).0)
        .await
        .expect("open")
}

fn token_calls(rig: &Rig) -> usize {
    rig.google
        .issuer
        .events()
        .iter()
        .filter(|e| matches!(e, IssuerEvent::Token { .. }))
        .count()
}

#[tokio::test]
async fn a_token_for_a_calendar_grant_is_a_live_bearer_with_the_calendar_scope() {
    let rig = Rig::new(PLAIN).await;
    let session = open(&rig).await;
    let calendar_row = rig
        .spec
        .capabilities
        .iter()
        .find(|r| r.family == porter_core::Family::GoogleCalendar)
        .and_then(|r| r.endpoint.clone())
        .expect("the calendar row's endpoint");
    for name in ["google_calendar", calendar_row.0.as_str()] {
        let token = session.access_token(&audience(name)).await.expect(name);
        assert_eq!(token.kind, TokenKind::Bearer, "{name}");
        assert!(rig.google.issuer.access_is_live(token.value.expose()));
        assert_eq!(token.expires, UnixSeconds(1_000_000 + 3600));
        let scope = rig
            .google
            .issuer
            .access_scope(token.value.expose())
            .expect("issued scope");
        assert!(scope.contains("auth/calendar"), "{scope}");
    }
    for foreign in [
        "graph",
        "webdav",
        "https://graph.evil.test",
        "google_calendar_evil",
    ] {
        assert_eq!(
            session.access_token(&audience(foreign)).await,
            Err(ProviderError::Forbidden),
            "{foreign}"
        );
    }
}

const CALENDAR: &str = "https://www.googleapis.com/auth/calendar";
const TASKS: &str = "https://www.googleapis.com/auth/tasks";

#[tokio::test]
async fn a_grants_refresh_carries_its_own_kinds_scope_alone() {
    let rig = Rig::new(PLAIN).await;
    let session = open(&rig).await;
    let token = session
        .access_token_for(&audience("google_calendar"), K::Calendar)
        .await
        .expect("calendar");
    assert_eq!(token.kind, TokenKind::Bearer);
    assert!(rig.google.issuer.access_is_live(token.value.expose()));
    assert_eq!(rig.wire.refresh_scopes(), [Some(CALENDAR.to_owned())]);

    session
        .access_token_for(&audience("google_tasks"), K::Tasks)
        .await
        .expect("tasks");
    session
        .access_token_for(&audience("google_drive"), K::Storage)
        .await
        .expect("drive");
    assert_eq!(
        rig.wire.refresh_scopes(),
        [
            Some(CALENDAR.to_owned()),
            Some(TASKS.to_owned()),
            Some("https://www.googleapis.com/auth/drive.appdata".to_owned()),
        ],
        "each kind its own scope, never the whole grant"
    );
    // A grant reaches only its own kind's rows.
    for (name, kind) in [
        ("google_drive", K::Calendar),
        ("google_calendar", K::Tasks),
        ("imap", K::Mail),
        ("google_calendar", K::Mail),
    ] {
        assert_eq!(
            session.access_token_for(&audience(name), kind).await,
            Err(ProviderError::Forbidden),
            "{name} {kind:?}"
        );
    }
    // Photos asks for the one scope its API takes: a person may have ticked only the picker.
    let fresh = open(&rig).await;
    let _ = fresh
        .access_token_for(&audience("google_photos_picker"), K::Photos)
        .await;
    assert_eq!(
        rig.wire.refresh_scopes().last(),
        Some(&Some(
            "https://www.googleapis.com/auth/photospicker.mediaitems.readonly".to_owned()
        ))
    );
}

#[tokio::test]
async fn a_grants_refresh_never_asks_beyond_what_was_granted() {
    let rig = Rig::new(PLAIN).await;
    let session = open(&rig).await;
    // Porter's own token: the answer says what the grant holds (no contacts).
    session
        .access_token(&audience("google_tasks"))
        .await
        .expect("own token");
    assert_eq!(rig.wire.refresh_scopes(), [None]);
    assert_eq!(
        session
            .access_token_for(&audience("google_people"), K::Contacts)
            .await,
        Err(ProviderError::Forbidden)
    );
    assert_eq!(
        rig.wire.refresh_scopes(),
        [None],
        "no refresh named a scope the grant does not hold"
    );
    session
        .access_token_for(&audience("google_tasks"), K::Tasks)
        .await
        .expect("tasks is in the grant");
    assert_eq!(rig.wire.refresh_scopes(), [None, Some(TASKS.to_owned())]);
}

#[tokio::test]
async fn mail_audiences_are_only_for_a_persons_own_client() {
    let plain = Rig::new(PLAIN).await;
    let session = open(&plain).await;
    for name in ["imap", "smtp"] {
        assert_eq!(
            session.access_token(&audience(name)).await,
            Err(ProviderError::Forbidden),
            "{name}"
        );
    }
    let own = Rig::new(BYO).await;
    let session = open(&own).await;
    for name in ["imap", "smtp"] {
        let token = session.access_token(&audience(name)).await.expect(name);
        assert_eq!(token.kind, TokenKind::Xoauth2, "{name}");
        let bearer = token
            .value
            .expose()
            .strip_prefix("user=ada@gmail.com\u{1}auth=Bearer ")
            .and_then(|rest| rest.strip_suffix("\u{1}\u{1}"))
            .unwrap_or_else(|| panic!("{name}: not the unencoded SASL string"));
        assert!(own.google.issuer.access_is_live(bearer));
    }
}

#[tokio::test]
async fn a_token_is_reused_until_due_and_renewed_with_the_same_refresh_token() {
    let rig = Rig::new(PLAIN).await;
    let (presented, refresh) = seeded(&rig);
    let session = rig
        .provider
        .open(&account(), presented)
        .await
        .expect("open");
    let first = session
        .access_token(&audience("google_tasks"))
        .await
        .expect("token");
    let again = session
        .access_token(&audience("google_calendar"))
        .await
        .expect("token");
    assert_eq!(first, again, "one token serves every service");
    assert_eq!(token_calls(&rig), 1);
    // Google does not rotate: nothing new to store, and the refresh token is still live.
    assert_eq!(session.renewed(), None);

    rig.advance(3600 - 30);
    let renewed = session
        .access_token(&audience("google_tasks"))
        .await
        .expect("token");
    assert_ne!(renewed.value, first.value);
    assert_eq!(renewed.expires, UnixSeconds(1_000_000 + 3600 - 30 + 3600));
    assert!(rig.google.issuer.access_is_live(renewed.value.expose()));
    assert_eq!(token_calls(&rig), 2);
    assert_eq!(session.renewed(), None);
    assert!(rig.google.issuer.refresh_is_live(&refresh));
}

#[tokio::test]
async fn a_refresh_google_refuses_needs_reauth_and_an_unreachable_one_does_not() {
    let rig = Rig::new(PLAIN).await;
    let session = open(&rig).await;
    rig.google.issuer.refuse_refreshes(1);
    assert_eq!(
        session.access_token(&audience("google_tasks")).await,
        Err(ProviderError::Unauthorized)
    );
    // A client whose app is verified: the person or the password ended it, not the seven days.
    assert_eq!(session.reauth_reason(), Some(ReauthReason::Revoked));
    // A good refresh clears the reason.
    session
        .access_token(&audience("google_tasks"))
        .await
        .expect("token");
    assert_eq!(session.reauth_reason(), None);

    let down = Rig::new(PLAIN).await;
    let session = open(&down).await;
    drop(down.google);
    assert_eq!(
        session.access_token(&audience("google_tasks")).await,
        Err(ProviderError::Unreachable)
    );
    assert_eq!(session.reauth_reason(), None);
}

#[tokio::test]
async fn a_client_in_testing_is_signed_out_after_seven_days_and_the_reason_says_so() {
    let rig = Rig::new(TESTING).await;
    let session = open(&rig).await;
    session
        .access_token(&audience("google_tasks"))
        .await
        .expect("token");
    // Seven days on, Google refuses the refresh as it does for every app in testing.
    rig.advance(7 * 86_400);
    rig.google.issuer.refuse_refreshes(1);
    assert_eq!(
        session.access_token(&audience("google_tasks")).await,
        Err(ProviderError::Unauthorized)
    );
    let reason = session.reauth_reason().expect("a reason");
    assert_eq!(reason, ReauthReason::TestingExpiry);
    assert_eq!(
        reason.plain(),
        "Google signs this app out every 7 days until it is verified."
    );
}

#[tokio::test]
async fn a_young_sign_in_refused_is_not_blamed_on_the_seven_days() {
    let rig = Rig::new(TESTING).await;
    let session = open(&rig)
        .await
        .signed_in_at(UnixSeconds(1_000_000 - 86_400));
    rig.google.issuer.refuse_refreshes(1);
    assert!(
        session
            .access_token(&audience("google_tasks"))
            .await
            .is_err()
    );
    assert_eq!(session.reauth_reason(), Some(ReauthReason::Revoked));
}

#[tokio::test]
async fn removing_the_account_revokes_the_refresh_token_at_google() {
    let rig = Rig::new(PLAIN).await;
    let (presented, refresh) = seeded(&rig);
    assert!(rig.google.issuer.refresh_is_live(&refresh));
    let outcome = rig
        .provider
        .revoke(&held_account(), &presented)
        .await
        .expect("revoke");
    assert_eq!(outcome, RevokeOutcome::Revoked);
    assert!(!rig.google.issuer.refresh_is_live(&refresh));
    assert!(rig.google.issuer.events().iter().any(|e| matches!(
        e,
        IssuerEvent::Revoke { token, known: true } if *token == refresh
    )));
    // Revoking again is not an error: the token is gone either way.
    assert_eq!(
        rig.provider
            .revoke(&held_account(), &presented)
            .await
            .expect("revoke"),
        RevokeOutcome::Revoked
    );
    // No client, nothing to ask.
    let none = Rig::new(Row::Absent).await;
    assert_eq!(
        none.provider
            .revoke(&held_account(), &presented)
            .await
            .expect("revoke"),
        RevokeOutcome::Unsupported
    );
}

#[tokio::test]
async fn discovery_on_reconnect_reads_the_same_claims_as_the_sign_in() {
    let rig = Rig::new(PLAIN).await;
    let (presented, _) = seeded(&rig);
    let claims = rig
        .provider
        .discover(&held_account(), &presented)
        .await
        .expect("claims");
    let present: Vec<K> = claims
        .iter()
        .filter(|c| matches!(c.offer, Offer::Present(_)))
        .map(|c| c.offer.kind())
        .collect();
    // The seeded grant has calendar, tasks and Drive's app folder (and Gmail, which this
    // client does not use); what it has not is not offered.
    assert_eq!(present, [K::Calendar, K::Tasks, K::Storage]);
    assert!(
        claims
            .iter()
            .any(|c| matches!(c.offer, Offer::Absent { kind: K::Mail, .. }))
    );
}

#[tokio::test]
async fn an_install_with_no_client_is_unreachable_not_signed_out() {
    let rig = Rig::new(Row::Absent).await;
    let session = open(&rig).await;
    assert_eq!(
        session.access_token(&audience("google_tasks")).await,
        Err(ProviderError::Unreachable)
    );
}
