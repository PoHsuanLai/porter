use super::rig::*;
use porter_core::capability::CapabilityKind as K;
use porter_core::sheet::{ServiceChoice, SignInFault, SignInInput};
use porter_core::{
    AbsentReason, Credential, Family, Offer, Provenance, SecretPurpose, TenantConsent, Toggle,
};
use porter_fake_servers::{Consent, IssuerEvent, TokenResult};
use porter_families::SignInFlow;
use porter_provider::{Provider, SignIn, SignInStep};

fn grant_types(rig: &Rig) -> Vec<String> {
    rig.issuer
        .events()
        .into_iter()
        .filter_map(|e| match e {
            IssuerEvent::Token {
                grant_type,
                result: TokenResult::Issued { .. },
                ..
            } => Some(grant_type),
            _ => None,
        })
        .collect()
}

/// Signs in by loopback up to the review, with the browser answering.
async fn to_review(rig: &Rig) -> (porter_families::MicrosoftSignIn<Wire>, SignInStep) {
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

#[tokio::test]
async fn a_loopback_sign_in_reviews_what_graph_allows_then_hands_over_the_credential() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
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
    assert_eq!(label.0, "ada@contoso.onmicrosoft.com");
    let kinds: Vec<K> = claims.iter().map(|c| c.offer.kind()).collect();
    assert_eq!(
        kinds,
        [
            K::Mail,
            K::Calendar,
            K::Contacts,
            K::Tasks,
            K::Notes,
            K::Storage
        ]
    );
    for claim in &claims {
        let want = match claim.offer.kind() {
            K::Mail => Provenance::Discovered,
            _ => Provenance::Probed,
        };
        assert_eq!(claim.provenance, want, "{claim:?}");
        assert!(matches!(claim.offer, Offer::Present(_)), "{claim:?}");
    }
    assert_eq!(restriction.consent, TenantConsent::User);
    let servers: Vec<(Family, &str)> = endpoints
        .iter()
        .map(|e| (e.family, e.url.as_str()))
        .collect();
    assert_eq!(
        servers,
        [
            (Family::Imap, "imaps://outlook.office365.com:993"),
            (Family::Smtp, "smtp://smtp.office365.com:587"),
            (Family::Graph, "https://graph.microsoft.com"),
        ]
    );
    assert!(endpoints.iter().all(|e| e.check().is_ok()));
    assert!(endpoints.iter().all(|e| e.login.0 == label.0));

    // Nothing is handed over before the person confirms.
    let again = signin.next(SignInInput::Poll).await;
    assert!(matches!(again, SignInStep::Review { .. }));
    let choices = vec![ServiceChoice {
        kind: K::Notes,
        toggle: Toggle::Off,
    }];
    let SignInStep::Done(signed) = signin.next(SignInInput::Confirm(choices)).await else {
        panic!("expected done")
    };
    let [(purpose, Credential::OAuth { refresh, .. })] = signed.credentials.as_slice() else {
        panic!("one OAuth credential: {:?}", signed.credentials)
    };
    assert_eq!(*purpose, SecretPurpose::OAuthRefresh);
    assert!(rig.issuer.refresh_is_live(refresh.expose()));

    // The code was redeemed for Exchange, and Graph's token came from a scoped refresh.
    assert_eq!(grant_types(&rig), ["authorization_code", "refresh_token"]);
    let calls = rig.graph.calls();
    assert_eq!(calls[0].0, "/v1.0/me");
    assert!(
        calls
            .iter()
            .all(|(_, bearer)| rig.issuer.access_is_live(bearer))
    );
    assert!(
        calls
            .iter()
            .any(|(path, _)| path == "/v1.0/me/onenote/notebooks")
    );
}

#[tokio::test]
async fn a_tenant_that_forbids_onenote_shows_notes_absent_for_consent() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    rig.graph.refuse("/v1.0/me/onenote/notebooks", 403);
    let (_, step) = to_review(&rig).await;
    let SignInStep::Review {
        claims,
        restriction,
        endpoints,
        ..
    } = step
    else {
        panic!("expected a review, got {step:?}")
    };
    let notes = claims
        .iter()
        .find(|c| c.offer.kind() == K::Notes)
        .expect("notes");
    assert_eq!(
        notes.offer,
        Offer::Absent {
            kind: K::Notes,
            reason: AbsentReason::TenantConsent
        }
    );
    assert_eq!(notes.provenance, Provenance::Probed);
    assert_eq!(restriction.consent, TenantConsent::AdminRequired);
    // The rest still answered, so Graph is still an endpoint.
    assert!(endpoints.iter().any(|e| e.family == Family::Graph));
    let present = claims
        .iter()
        .filter(|c| matches!(c.offer, Offer::Present(_)))
        .count();
    assert_eq!(present, 5);
}

#[tokio::test]
async fn a_personal_account_that_is_refused_has_no_tenant_to_blame() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    *rig.graph.address.lock().unwrap() = "ada@outlook.com".into();
    rig.graph.refuse("/v1.0/me/onenote/notebooks", 403);
    rig.graph.refuse("/v1.0/me/todo/lists", 404);
    let (_, step) = to_review(&rig).await;
    let SignInStep::Review {
        claims,
        restriction,
        ..
    } = step
    else {
        panic!("expected a review, got {step:?}")
    };
    let reason_of = |kind| {
        claims.iter().find_map(|c| match &c.offer {
            Offer::Absent { kind: k, reason } if *k == kind => Some(*reason),
            _ => None,
        })
    };
    assert_eq!(reason_of(K::Notes), Some(AbsentReason::ProviderOffersNone));
    assert_eq!(reason_of(K::Tasks), Some(AbsentReason::NotOnServer));
    assert_eq!(restriction.consent, TenantConsent::User);
}

#[tokio::test]
async fn onedrive_reports_its_quota() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    let (_, step) = to_review(&rig).await;
    let SignInStep::Review { claims, .. } = step else {
        panic!("expected a review")
    };
    let storage = claims
        .iter()
        .find(|c| c.offer.kind() == K::Storage)
        .expect("storage");
    let Offer::Present(porter_core::capability::Capability::Storage(cap)) = &storage.offer else {
        panic!("present storage")
    };
    assert_eq!(cap.quota, porter_core::capability::QuotaReport::Reported);
}

#[tokio::test]
async fn declining_the_consent_page_cancels_and_nothing_is_exchanged() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    rig.issuer.set_consent(Consent::Deny);
    let (_, step) = to_review(&rig).await;
    assert_eq!(step, SignInStep::Failed(SignInFault::Cancelled));
    assert!(grant_types(&rig).is_empty());
}

#[tokio::test]
async fn a_build_with_no_client_id_says_so_before_opening_anything() {
    let rig = Rig::new(SignInFlow::Loopback, false).await;
    let mut signin = rig.provider.sign_in(start()).expect("sign in");
    assert_eq!(
        signin.next(SignInInput::Start).await,
        SignInStep::Failed(SignInFault::NeedsClientId)
    );
    assert!(rig.issuer.events().is_empty());
}

#[tokio::test]
async fn cancelling_closes_the_listeners_and_ends_the_sign_in() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    let mut signin = rig.provider.sign_in(start()).expect("sign in");
    let SignInStep::OpenBrowser { url } = signin.next(SignInInput::Start).await else {
        panic!("expected the browser")
    };
    assert_eq!(signin.next(SignInInput::Poll).await, SignInStep::Waiting);
    let port = redirect_port(url.as_str()).expect("the redirect names a port");
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
    );
    assert_eq!(
        signin.next(SignInInput::Cancel).await,
        SignInStep::Failed(SignInFault::Cancelled)
    );
    drop(signin);
    // The listener's task is aborted with the sign-in; give the runtime a turn to close it.
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the loopback listener is still listening");
}

#[tokio::test]
async fn a_device_code_sign_in_shows_the_code_then_reviews_once_it_is_approved() {
    let rig = Rig::new(SignInFlow::DeviceCode, true).await;
    let mut signin = rig.provider.sign_in(start()).expect("sign in");
    let SignInStep::ShowCode { user_code, url } = signin.next(SignInInput::Start).await else {
        panic!("expected a code")
    };
    assert!(url.as_str().ends_with("/activate"));
    // Not approved yet: the poll interval (a second on the issuer's clock) has to pass first.
    assert_eq!(signin.next(SignInInput::Poll).await, SignInStep::Waiting);
    rig.advance(2);
    assert_eq!(signin.next(SignInInput::Poll).await, SignInStep::Waiting);
    rig.issuer.approve_device(&user_code.0);
    rig.advance(2);
    let step = until_settled(&mut signin, 5).await;
    assert!(matches!(step, SignInStep::Review { .. }), "{step:?}");
    let done = signin.next(SignInInput::Confirm(Vec::new())).await;
    assert!(matches!(done, SignInStep::Done(_)));
}

#[tokio::test]
async fn a_denied_device_code_cancels_and_an_expired_one_times_out() {
    let rig = Rig::new(SignInFlow::DeviceCode, true).await;
    let mut signin = rig.provider.sign_in(start()).expect("sign in");
    let SignInStep::ShowCode { user_code, .. } = signin.next(SignInInput::Start).await else {
        panic!("expected a code")
    };
    rig.issuer.deny_device(&user_code.0);
    rig.advance(2);
    assert_eq!(
        until_settled(&mut signin, 5).await,
        SignInStep::Failed(SignInFault::Cancelled)
    );

    let mut late = rig.provider.sign_in(start()).expect("sign in");
    assert!(matches!(
        late.next(SignInInput::Start).await,
        SignInStep::ShowCode { .. }
    ));
    rig.advance(10_000);
    assert_eq!(
        late.next(SignInInput::Poll).await,
        SignInStep::Failed(SignInFault::TimedOut)
    );
}

#[tokio::test]
async fn a_graph_that_answers_5xx_ends_the_sign_in_unreachable_and_it_stays_over() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    let mut signin = rig.provider.sign_in(start()).expect("sign in");
    let SignInStep::OpenBrowser { url } = signin.next(SignInInput::Start).await else {
        panic!("expected the browser")
    };
    let page = url.as_str().to_owned();
    let browser = tokio::spawn(async move { browse(&page).await });
    // The issuer has issued; Graph is the part that is down.
    *rig.graph.statuses.lock().unwrap() = [("/v1.0/me".to_owned(), 503)].into();
    let step = until_settled(&mut signin, 200).await;
    browser.await.expect("task").expect("browser");
    assert_eq!(step, SignInStep::Failed(SignInFault::Unreachable));
    assert_eq!(
        signin.next(SignInInput::Poll).await,
        SignInStep::Failed(SignInFault::Cancelled)
    );
}
