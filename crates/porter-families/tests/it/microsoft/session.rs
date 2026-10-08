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

fn held_account() -> porter_core::Account {
    porter_core::Account {
        id: account(),
        provider: porter_core::ProviderId::parse("microsoft").expect("provider"),
        label: porter_core::AccountLabel("ada".into()),
        state: porter_core::AccountState::Ok,
        auth: porter_core::AuthKind::OAuthPkce,
        capabilities: vec![],
        restriction: porter_core::Restriction::none(),
        endpoints: vec![],
    }
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

#[tokio::test]
async fn a_calendar_grants_token_request_names_only_the_calendar_scope() {
    const CALENDARS: &str = "https://graph.microsoft.com/Calendars.ReadWrite";
    let rig = Rig::new(SignInFlow::Loopback, true).await;
    let session = open(&rig).await;
    for audience in ["graph", GRAPH] {
        let token = session
            .access_token_for(&audience_of(audience), K::Calendar)
            .await
            .expect(audience);
        assert_eq!(token.kind, TokenKind::Bearer, "{audience}");
        assert!(
            rig.issuer.access_is_live(token.value.expose()),
            "{audience}"
        );
    }
    // One refresh (the second audience reuses the fresh token), naming the calendar alone.
    assert_eq!(rig.graph.refresh_scopes(), [Some(CALENDARS.to_owned())]);

    // Each kind its own scope, never Graph's `.default`; mail only over IMAP and SMTP.
    for (kind, scope) in [
        (
            K::Contacts,
            "https://graph.microsoft.com/Contacts.ReadWrite",
        ),
        (K::Tasks, "https://graph.microsoft.com/Tasks.ReadWrite"),
        (K::Notes, "https://graph.microsoft.com/Notes.ReadWrite"),
        (
            K::Storage,
            "https://graph.microsoft.com/Files.ReadWrite.AppFolder",
        ),
    ] {
        session
            .access_token_for(&audience_of("graph"), kind)
            .await
            .expect("token");
        assert_eq!(
            rig.graph.refresh_scopes().last(),
            Some(&Some(scope.to_owned())),
            "{kind:?}"
        );
    }
    let mail = session
        .access_token_for(&audience_of("imap"), K::Mail)
        .await
        .expect("imap");
    assert_eq!(mail.kind, TokenKind::Xoauth2);
    for (audience, kind) in [
        ("graph", K::Mail),
        ("imap", K::Calendar),
        ("smtp", K::Storage),
        ("https://graph.evil.test", K::Calendar),
    ] {
        assert_eq!(
            session.access_token_for(&audience_of(audience), kind).await,
            Err(ProviderError::Forbidden),
            "{audience} {kind:?}"
        );
    }
    // No refresh for a grant named Graph's `.default`; the mailbox address for XOAUTH2 was read
    // with the profile scope alone.
    let scopes = rig.graph.refresh_scopes();
    assert!(
        scopes
            .iter()
            .all(|s| s.as_deref() != Some("https://graph.microsoft.com/.default")),
        "{scopes:?}"
    );
    assert!(scopes.contains(&Some("https://graph.microsoft.com/User.Read".to_owned())));
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
        .discover(&held_account(), &seeded(&rig))
        .await
        .expect("claims");
    assert!(before.iter().all(|c| matches!(c.offer, Offer::Present(_))));

    rig.graph.refuse("/v1.0/me/onenote/notebooks", 403);
    let after = rig
        .provider
        .discover(&held_account(), &seeded(&rig))
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
    let outcome = rig
        .provider
        .revoke(&held_account(), &seeded(&rig))
        .await
        .expect("outcome");
    let RevokeOutcome::Manual(page) = outcome else {
        panic!("expected a page")
    };
    assert_eq!(page.origin().host, "account.microsoft.com");
    assert!(rig.issuer.events().is_empty());
}

mod service {
    use super::*;
    use porter_core::AccountId as Id;
    use porter_core::{
        Account, AccountLabel, AccountState, AuthKind, ProviderId, Restriction, SecretKey,
        SecretPurpose,
    };
    use porter_fake::{FixedClock, ScriptedSheets};
    use porter_secrets::{MemorySecrets, Secrets, SecretsError};
    use porter_service::{AccountService, Registry};
    use std::sync::Arc;

    /// A store the test keeps a handle on after the service has one.
    #[derive(Debug, Clone, Default)]
    struct Shared(Arc<MemorySecrets>);

    impl Secrets for Shared {
        async fn put(&self, key: &SecretKey, value: &Credential) -> Result<(), SecretsError> {
            self.0.put(key, value).await
        }
        async fn get(&self, key: &SecretKey) -> Result<Credential, SecretsError> {
            self.0.get(key).await
        }
        async fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
            self.0.delete(key).await
        }
        async fn delete_account(&self, account: &Id) -> Result<(), SecretsError> {
            self.0.delete_account(account).await
        }
    }

    fn held() -> Account {
        Account {
            id: account(),
            provider: ProviderId::parse("microsoft").expect("provider"),
            label: AccountLabel("ada@contoso.onmicrosoft.com".into()),
            state: AccountState::Ok,
            auth: AuthKind::OAuthPkce,
            capabilities: vec![],
            restriction: Restriction::none(),
            endpoints: vec![],
        }
    }

    fn refresh_of(credential: &Credential) -> String {
        match credential {
            Credential::OAuth { refresh, .. } => refresh.expose().to_owned(),
            other => panic!("not an OAuth credential: {other:?}"),
        }
    }

    #[tokio::test]
    async fn rediscovering_stores_the_refresh_token_the_issuer_rotated() {
        let rig = Rig::new(SignInFlow::Loopback, true).await;
        let Presented::Credential(old) = seeded(&rig) else {
            panic!("a credential")
        };
        let key = SecretKey {
            account: account(),
            purpose: SecretPurpose::OAuthRefresh,
        };
        let secrets = Shared::default();
        secrets.put(&key, &old).await.expect("put");
        let service = AccountService::new(
            vec![rig.provider.clone()],
            Registry {
                accounts: vec![held()],
                ..Registry::default()
            },
            secrets.clone(),
            ScriptedSheets::answering(vec![]),
            FixedClock(porter_fake::NOW),
        );

        let claims = service.rediscover(&account()).await.expect("claims");
        assert!(!claims.is_empty());
        assert!(claims.iter().all(|c| matches!(c.offer, Offer::Present(_))));
        assert_eq!(
            service.registry().accounts[0].capabilities.len(),
            claims.len()
        );

        // The issuer replaced the token it was given; the new one is what is filed, and it is
        // live: discovering again from the stored token works.
        let stored = secrets.get(&key).await.expect("stored");
        assert_ne!(refresh_of(&stored), refresh_of(&old), "the token rotated");
        assert!(!rig.issuer.refresh_is_live(&refresh_of(&old)));
        assert!(rig.issuer.refresh_is_live(&refresh_of(&stored)));
        service.rediscover(&account()).await.expect("again");
        let after = secrets.get(&key).await.expect("stored");
        assert!(rig.issuer.refresh_is_live(&refresh_of(&after)));
    }

    #[tokio::test]
    async fn a_refused_refresh_token_is_a_sign_in_again_and_nothing_is_stored() {
        let rig = Rig::new(SignInFlow::Loopback, true).await;
        let key = SecretKey {
            account: account(),
            purpose: SecretPurpose::OAuthRefresh,
        };
        let secrets = Shared::default();
        let dead = Credential::OAuth {
            access: SecretText::new("stale"),
            refresh: SecretText::new("never-issued"),
            expires_at: UnixSeconds(0),
        };
        secrets.put(&key, &dead).await.expect("put");
        let service = AccountService::new(
            vec![rig.provider.clone()],
            Registry {
                accounts: vec![held()],
                ..Registry::default()
            },
            secrets.clone(),
            ScriptedSheets::answering(vec![]),
            FixedClock(porter_fake::NOW),
        );
        assert_eq!(
            service.rediscover(&account()).await,
            Err(porter_core::wire::Refusal::NeedsReauth)
        );
        assert_eq!(secrets.get(&key).await, Ok(dead));
    }
}
