use super::rig::*;
use porter_core::capability::CapabilityKind as K;
use porter_core::{
    AbsentReason, AccountId, Credential, Offer, Provenance, SecretText, TokenKind, UnixSeconds,
};
use porter_families::SignInFlow;
use porter_provider::{Presented, Provider, ProviderError, ProviderSession, RevokeOutcome};

fn account() -> AccountId {
    AccountId::parse("11111111-1111-4111-8111-111111111111").expect("account id")
}

fn seeded(rig: &Rig) -> Presented {
    let refresh = rig.issuer.seed_refresh(CLIENT_ID, "offline_access");
    Presented::Credential(Credential::OAuth {
        access: SecretText::new("stale"),
        refresh: SecretText::new(refresh),
        expires_at: UnixSeconds(0),
    })
}

async fn open(rig: &Rig) -> porter_families::MicrosoftSession<Wire> {
    rig.provider
        .open(&account(), seeded(rig))
        .await
        .expect("open")
}

#[tokio::test]
async fn tokens_come_per_audience_in_the_form_the_protocol_takes() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    let session = open(&rig).await;
    for (audience, kind) in [
        ("imap", TokenKind::Xoauth2),
        ("smtp", TokenKind::Xoauth2),
        ("graph", TokenKind::Bearer),
        ("https://graph.microsoft.com", TokenKind::Bearer),
    ] {
        let token = session
            .access_token(&audience_of(audience))
            .await
            .expect(audience);
        assert_eq!(token.kind, kind, "{audience}");
        let bearer = match kind {
            TokenKind::Xoauth2 => token
                .value
                .expose()
                .strip_prefix("user=ada@contoso.onmicrosoft.com\u{1}auth=Bearer ")
                .and_then(|rest| rest.strip_suffix("\u{1}\u{1}"))
                .unwrap_or_else(|| panic!("{audience}: not the unencoded SASL string")),
            _ => token.value.expose(),
        };
        assert!(rig.issuer.access_is_live(bearer), "{audience}");
        assert_eq!(token.expires, UnixSeconds(1_000_000 + 3600), "{audience}");
    }
    assert_eq!(
        session.access_token(&audience_of("webdav")).await,
        Err(ProviderError::Forbidden)
    );
    assert_eq!(
        session
            .access_token(&audience_of("https://graph.evil.test"))
            .await,
        Err(ProviderError::Forbidden)
    );
}

fn audience_of(text: &str) -> porter_core::Audience {
    audience(text)
}

#[tokio::test]
async fn a_token_is_reused_until_it_is_due_and_then_renewed_with_the_rotated_refresh() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    let session = open(&rig).await;
    let first = session
        .access_token(&audience_of("imap"))
        .await
        .expect("token");
    let again = session
        .access_token(&audience_of("imap"))
        .await
        .expect("token");
    assert_eq!(first, again);
    let calls = |rig: &Rig| {
        rig.issuer
            .events()
            .iter()
            .filter(|e| matches!(e, porter_fake_servers::IssuerEvent::Token { .. }))
            .count()
    };
    // The IMAP token, and the Graph token that named the mailbox.
    assert_eq!(calls(&rig), 2);

    // The rotated refresh token from the first renewal is to be stored, once.
    let stored = session.renewed().expect("the refresh token rotated");
    let Credential::OAuth { refresh, .. } = &stored else {
        panic!("an OAuth credential")
    };
    assert!(rig.issuer.refresh_is_live(refresh.expose()));
    assert_eq!(session.renewed(), None);

    rig.advance(3600 - 30);
    let renewed = session
        .access_token(&audience_of("imap"))
        .await
        .expect("token");
    assert_ne!(renewed.value, first.value);
    assert_eq!(calls(&rig), 3);
    let Credential::OAuth {
        refresh: second, ..
    } = session.renewed().expect("rotated again")
    else {
        panic!("an OAuth credential")
    };
    assert_ne!(&second, refresh);
}

#[tokio::test]
async fn a_refused_grant_needs_reauthentication_and_a_down_network_does_not() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    let session = open(&rig).await;
    rig.issuer.refuse_refreshes(1);
    assert_eq!(
        session.access_token(&audience_of("imap")).await,
        Err(ProviderError::Unauthorized)
    );
    *rig.graph.down.lock().unwrap() = true;
    assert_eq!(
        session.access_token(&audience_of("imap")).await,
        Err(ProviderError::Unreachable)
    );
}

#[tokio::test]
async fn opening_needs_an_oauth_credential_and_a_registered_client_to_renew() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    for presented in [
        Presented::Anonymous,
        Presented::Credential(Credential::Password(SecretText::new("pw"))),
    ] {
        assert!(matches!(
            rig.provider.open(&account(), presented).await,
            Err(ProviderError::Unauthorized)
        ));
    }
    let bare = Rig::new(SignInFlow::Loopback, false).await;
    let session = open(&bare).await;
    assert_eq!(
        session.access_token(&audience_of("imap")).await,
        Err(ProviderError::Unreachable)
    );
}

#[tokio::test]
async fn discover_probes_graph_again_and_shows_a_tenant_change() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    let before = rig
        .provider
        .discover(&account(), &seeded(&rig))
        .await
        .expect("claims");
    assert!(before.iter().all(|c| matches!(c.offer, Offer::Present(_))));

    rig.graph.refuse("/v1.0/me/onenote/notebooks", 403);
    let after = rig
        .provider
        .discover(&account(), &seeded(&rig))
        .await
        .expect("claims");
    let changed: Vec<_> = before.iter().zip(&after).filter(|(a, b)| a != b).collect();
    assert_eq!(changed.len(), 1);
    assert_eq!(
        changed[0].1.offer,
        Offer::Absent {
            kind: K::Notes,
            reason: AbsentReason::TenantConsent
        }
    );
    assert_eq!(changed[0].1.provenance, Provenance::Probed);
}

#[tokio::test]
async fn revoking_sends_the_person_to_their_microsoft_account_page() {
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    let outcome = rig.provider.revoke(&seeded(&rig)).await.expect("outcome");
    let RevokeOutcome::Manual(page) = outcome else {
        panic!("expected a page")
    };
    assert_eq!(page.origin().host, "account.microsoft.com");
    assert!(rig.issuer.events().is_empty());
}
