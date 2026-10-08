use super::rig::*;
use porter_core::capability::CapabilityKind as K;
use porter_core::sheet::{ServiceChoice, SignInFault, SignInInput};
use porter_core::{
    AbsentReason, Count, Credential, Family, LimitReason, Offer, Provenance, SecretPurpose, Toggle,
    TokenLifetime, Verification,
};
use porter_fake_servers::{Consent, IssuerEvent, TokenResult};
use porter_provider::{Provider, SignIn, SignInMode, SignInStart, SignInStep};

/// Signs in by loopback up to the review, with the browser answering.
async fn to_review(rig: &Rig) -> (porter_families::GoogleSignIn<Wire>, SignInStep) {
    let mut signin = rig.provider.sign_in(start()).expect("sign in");
    let SignInStep::OpenBrowser { url } = signin.next(SignInInput::Start).await else {
        panic!("expected the browser")
    };
    let page = url.as_str().to_owned();
    let browser = tokio::spawn(async move { browse(&page).await });
    let step = until_settled(&mut signin, 200).await;
    browser.await.expect("task").expect("browser");
    (signin, step)
}

fn review(step: SignInStep) -> (Vec<porter_core::Claim>, Vec<porter_core::ServiceEndpoint>) {
    let SignInStep::Review {
        claims, endpoints, ..
    } = step
    else {
        panic!("expected a review, got {step:?}")
    };
    (claims, endpoints)
}

fn offer_of(claims: &[porter_core::Claim], kind: K) -> &Offer {
    &claims
        .iter()
        .find(|c| c.offer.kind() == kind)
        .expect("a claim for the kind")
        .offer
}

fn query_of(rig: &Rig) -> Vec<(String, String)> {
    rig.google
        .issuer
        .authorize_queries()
        .pop()
        .expect("an authorize request")
}

fn get<'a>(query: &'a [(String, String)], name: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

#[tokio::test]
async fn a_loopback_sign_in_reviews_what_google_allows_then_hands_over_the_credential() {
    let rig = Rig::new(PLAIN).await;
    let (mut signin, step) = to_review(&rig).await;
    let SignInStep::Review {
        claims,
        endpoints,
        restriction,
        label,
    } = step
    else {
        panic!("expected a review, got {step:?}")
    };
    assert_eq!(label.0, "ada@gmail.com");
    let kinds: Vec<K> = claims.iter().map(|c| c.offer.kind()).collect();
    assert_eq!(
        kinds,
        [
            K::Mail,
            K::Calendar,
            K::Contacts,
            K::Tasks,
            K::Storage,
            K::Photos
        ]
    );
    // Mail's restricted scope is not asked of a client that is not the person's own.
    assert_eq!(
        offer_of(&claims, K::Mail),
        &Offer::Absent {
            kind: K::Mail,
            reason: AbsentReason::UnverifiedBuild
        }
    );
    for claim in claims.iter().filter(|c| c.offer.kind() != K::Mail) {
        let want = match claim.offer.kind() {
            K::Photos => Provenance::Discovered,
            _ => Provenance::Probed,
        };
        assert_eq!(claim.provenance, want, "{claim:?}");
        assert!(matches!(claim.offer, Offer::Present(_)), "{claim:?}");
    }
    let servers: Vec<(Family, String)> = endpoints
        .iter()
        .map(|e| (e.family, e.url.path().to_owned()))
        .collect();
    assert_eq!(
        servers,
        [
            (Family::GoogleCalendar, "/calendar/v3".to_owned()),
            (Family::GooglePeople, "/v1".to_owned()),
            (Family::GoogleTasks, "/tasks/v1".to_owned()),
            (Family::GoogleDrive, "/drive/v3".to_owned()),
            (Family::GooglePhotosUpload, "/v1".to_owned()),
            (Family::GooglePhotosPicker, "/v1".to_owned()),
        ]
    );
    assert!(endpoints.iter().all(|e| e.check().is_ok()));
    assert!(endpoints.iter().all(|e| e.login.0 == label.0));
    // The scopes narrow Drive and Photos, and a verified client has no cap or expiry.
    assert_eq!(restriction.verification, Verification::Verified);
    assert_eq!(restriction.token_lifetime, TokenLifetime::Standard);
    let limits: Vec<_> = restriction
        .limits
        .iter()
        .map(|l| (l.kind, l.reason))
        .collect();
    assert_eq!(
        limits,
        [
            (K::Storage, LimitReason::AppFolderOnly),
            (K::Photos, LimitReason::PickerOnly)
        ]
    );

    // Nothing is handed over before the person confirms.
    assert!(matches!(
        signin.next(SignInInput::Poll).await,
        SignInStep::Review { .. }
    ));
    let choices = vec![ServiceChoice {
        kind: K::Tasks,
        toggle: Toggle::Off,
    }];
    let SignInStep::Done(signed) = signin.next(SignInInput::Confirm(choices)).await else {
        panic!("expected done")
    };
    let [(purpose, Credential::OAuth { refresh, .. })] = signed.credentials.as_slice() else {
        panic!("one OAuth credential: {:?}", signed.credentials)
    };
    assert_eq!(*purpose, SecretPurpose::OAuthRefresh);
    assert!(rig.google.issuer.refresh_is_live(refresh.expose()));
    // One code exchange, which the issuer let through only because the application secret rode on it.
    let events = rig.google.issuer.events();
    let issued: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            IssuerEvent::Token {
                grant_type,
                result: TokenResult::Issued { .. },
                ..
            } => Some(grant_type.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(issued, ["authorization_code"]);
    // Every read carried a live bearer.
    let hits = rig.google.api.hits();
    assert!(hits.len() >= 5, "{hits:?}");
    assert!(hits.iter().all(|h| h.status == 200), "{hits:?}");
}

#[tokio::test]
async fn the_authorize_url_asks_offline_access_with_consent_and_one_scope_per_service() {
    let rig = Rig::new(PLAIN).await;
    let _ = to_review(&rig).await;
    let q = query_of(&rig);
    assert_eq!(get(&q, "client_id"), Some(CLIENT_ID));
    assert_eq!(get(&q, "access_type"), Some("offline"));
    assert_eq!(get(&q, "prompt"), Some("consent"));
    assert_eq!(get(&q, "include_granted_scopes"), Some("true"));
    assert_eq!(get(&q, "code_challenge_method"), Some("S256"));
    assert_eq!(get(&q, "response_type"), Some("code"));
    let scope = get(&q, "scope").expect("scope");
    for want in [
        "openid",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/calendar",
        "https://www.googleapis.com/auth/contacts",
        "https://www.googleapis.com/auth/tasks",
        "https://www.googleapis.com/auth/drive.appdata",
        "https://www.googleapis.com/auth/photoslibrary.appendonly",
        "https://www.googleapis.com/auth/photospicker.mediaitems.readonly",
    ] {
        assert!(scope.split(' ').any(|s| s == want), "{want} in {scope}");
    }
    assert!(!scope.contains("mail.google.com"), "{scope}");
    assert!(
        !scope.contains("auth/drive "),
        "the whole Drive is never asked: {scope}"
    );
}

#[tokio::test]
async fn a_persons_own_client_also_asks_gmail_and_gets_imap_and_smtp_endpoints() {
    let rig = Rig::new(BYO).await;
    let (_, step) = to_review(&rig).await;
    let q = query_of(&rig);
    assert!(
        get(&q, "scope")
            .expect("scope")
            .split(' ')
            .any(|s| s == "https://mail.google.com/")
    );
    let SignInStep::Review {
        claims,
        endpoints,
        restriction,
        ..
    } = step
    else {
        panic!("expected a review, got {step:?}")
    };
    assert!(matches!(
        offer_of(&claims, K::Mail),
        Offer::Present(porter_core::capability::Capability::Mail(_))
    ));
    let mail: Vec<(Family, String)> = endpoints
        .iter()
        .filter(|e| matches!(e.family, Family::Imap | Family::Smtp))
        .map(|e| (e.family, e.url.as_str().to_owned()))
        .collect();
    assert_eq!(
        mail,
        [
            (Family::Imap, "imaps://imap.gmail.com:993".to_owned()),
            (Family::Smtp, "smtp://smtp.gmail.com:587".to_owned()),
        ]
    );
    assert!(endpoints.iter().all(|e| e.check().is_ok()));
    // This is also a client in testing.
    assert_eq!(
        restriction.verification,
        Verification::Unverified {
            user_cap: Count(100)
        }
    );
    assert_eq!(restriction.token_lifetime, TokenLifetime::SevenDays);
}

#[tokio::test]
async fn with_no_client_row_the_sign_in_needs_a_client_id_and_dials_nothing() {
    let rig = Rig::new(Row::Absent).await;
    let mut signin = rig.provider.sign_in(start()).expect("sign in");
    assert_eq!(
        signin.next(SignInInput::Start).await,
        SignInStep::Failed(SignInFault::NeedsClientId)
    );
    assert!(rig.google.issuer.events().is_empty());
    assert!(rig.google.issuer.authorize_queries().is_empty());
    assert!(rig.google.api.hits().is_empty());
}

#[tokio::test]
async fn an_api_not_switched_on_and_a_refused_service_are_told_apart() {
    let rig = Rig::new(PLAIN).await;
    rig.google
        .api
        .disable_api("/calendar/v3/users/me/calendarList");
    rig.google.api.refuse("/tasks/v1/users/@me/lists", 403);
    rig.google.api.refuse("/v1/contactGroups", 404);
    let (_, step) = to_review(&rig).await;
    let (claims, endpoints) = review(step);
    let reason = |kind| match offer_of(&claims, kind) {
        Offer::Absent { reason, .. } => *reason,
        other => panic!("{kind:?}: {other:?}"),
    };
    // The owner can fix the first (enable the API); a personal account has no administrator.
    assert_eq!(reason(K::Calendar), AbsentReason::UnverifiedBuild);
    assert_eq!(reason(K::Contacts), AbsentReason::NotOnServer);
    assert!(matches!(offer_of(&claims, K::Storage), Offer::Present(_)));
    assert!(
        endpoints.iter().all(|e| e.family != Family::GoogleCalendar),
        "no endpoint for a service that is absent"
    );

    let work = Rig::new(PLAIN).await;
    work.google.api.set_user("ada@firm.example", "Ada");
    work.google.api.refuse("/tasks/v1/users/@me/lists", 403);
    let (_, step) = to_review(&work).await;
    let (claims, _) = review(step);
    assert_eq!(
        offer_of(&claims, K::Tasks),
        &Offer::Absent {
            kind: K::Tasks,
            reason: AbsentReason::TenantConsent
        }
    );
    let personal = Rig::new(PLAIN).await;
    personal.google.api.refuse("/tasks/v1/users/@me/lists", 403);
    let (_, step) = to_review(&personal).await;
    let (claims, _) = review(step);
    assert_eq!(
        offer_of(&claims, K::Tasks),
        &Offer::Absent {
            kind: K::Tasks,
            reason: AbsentReason::ProviderOffersNone
        }
    );
}

#[tokio::test]
async fn signing_in_again_is_done_without_a_review() {
    let rig = Rig::new(PLAIN).await;
    let mut signin = rig
        .provider
        .sign_in(SignInStart {
            mode: SignInMode::Reauthenticate {
                account: porter_core::AccountId::parse("ada").expect("id"),
                endpoints: Vec::new(),
            },
        })
        .expect("sign in");
    let SignInStep::OpenBrowser { url } = signin.next(SignInInput::Start).await else {
        panic!("expected the browser")
    };
    let page = url.as_str().to_owned();
    let browser = tokio::spawn(async move { browse(&page).await });
    let step = until_settled(&mut signin, 200).await;
    browser.await.expect("task").expect("browser");
    let SignInStep::Done(signed) = step else {
        panic!("expected done, got {step:?}")
    };
    assert_eq!(signed.credentials.len(), 1);
}

#[tokio::test]
async fn a_person_who_says_no_cancels_and_cancel_frees_the_listener() {
    let rig = Rig::new(PLAIN).await;
    rig.google.issuer.set_consent(Consent::Deny);
    let (_, step) = to_review(&rig).await;
    assert_eq!(step, SignInStep::Failed(SignInFault::Cancelled));

    let rig = Rig::new(PLAIN).await;
    let mut signin = rig.provider.sign_in(start()).expect("sign in");
    let SignInStep::OpenBrowser { .. } = signin.next(SignInInput::Start).await else {
        panic!("expected the browser")
    };
    assert_eq!(
        signin.next(SignInInput::Cancel).await,
        SignInStep::Failed(SignInFault::Cancelled)
    );
}

const UPLOAD_SCOPE: &str = "https://www.googleapis.com/auth/photoslibrary.appendonly";
const PICKER_SCOPE: &str = "https://www.googleapis.com/auth/photospicker.mediaitems.readonly";

/// What a sign-in where the person unticked `unticked` offers of Photos: whether it uploads,
/// what it reads, and the Photos endpoints it holds.
async fn photos_after_unticking(
    unticked: &[&str],
) -> (porter_core::capability::PhotosCap, Vec<Family>) {
    let rig = Rig::new(PLAIN).await;
    rig.google.issuer.untick(unticked);
    let (_, step) = to_review(&rig).await;
    let (claims, endpoints) = review(step);
    let Offer::Present(porter_core::capability::Capability::Photos(photos)) =
        offer_of(&claims, K::Photos).clone()
    else {
        panic!("Photos should be present")
    };
    let families = endpoints
        .iter()
        .map(|e| e.family)
        .filter(|f| matches!(f, Family::GooglePhotosUpload | Family::GooglePhotosPicker))
        .collect();
    (photos, families)
}

#[tokio::test]
async fn a_person_who_grants_only_the_picker_gets_no_upload_to_offer() {
    use porter_core::capability::{Albums, LibraryRead, Offered};
    let (photos, families) = photos_after_unticking(&[UPLOAD_SCOPE]).await;
    assert_eq!(photos.upload, Offered::Absent);
    assert_eq!(photos.albums, Albums::None);
    assert_eq!(photos.library_read, LibraryRead::PickerOnly);
    assert_eq!(families, [Family::GooglePhotosPicker]);
}

#[tokio::test]
async fn a_person_who_grants_only_the_upload_gets_no_picker_to_offer() {
    use porter_core::capability::{Albums, LibraryRead, Offered};
    let (photos, families) = photos_after_unticking(&[PICKER_SCOPE]).await;
    assert_eq!(photos.upload, Offered::Present);
    assert_eq!(photos.albums, Albums::AppCreated);
    assert_eq!(photos.library_read, LibraryRead::None);
    assert_eq!(families, [Family::GooglePhotosUpload]);
}

#[tokio::test]
async fn both_photos_scopes_offer_both_and_neither_turns_photos_off() {
    use porter_core::capability::Offered;
    let (photos, families) = photos_after_unticking(&[]).await;
    assert_eq!(photos.upload, Offered::Present);
    assert_eq!(
        families,
        [Family::GooglePhotosUpload, Family::GooglePhotosPicker]
    );
    let rig = Rig::new(PLAIN).await;
    rig.google.issuer.untick(&[UPLOAD_SCOPE, PICKER_SCOPE]);
    let (_, step) = to_review(&rig).await;
    let (claims, endpoints) = review(step);
    assert!(matches!(offer_of(&claims, K::Photos), Offer::Absent { .. }));
    assert!(!endpoints.iter().any(|e| matches!(
        e.family,
        Family::GooglePhotosUpload | Family::GooglePhotosPicker
    )));
}
