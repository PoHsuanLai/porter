//! The account service over its seams: the audience a grant covers, the endpoints a candidate
//! carries and a relay may dial, and what is saved and audited when the registry changes.

use porter_core::audit::AuditEvent;
use porter_core::capability::{Access, Delta, Offered, QuotaReport, StorageScope};
use porter_core::consent::{GrantScope, Usage};
use porter_core::need::{MailNeed, StorageNeed};
use porter_core::wire::ParentWindow;
use porter_core::wire::Refusal;
use porter_core::{
    AccountId, AccountsReply, AccountsRequest, AppId, AppName, Audience, CapabilityKind, DataClass,
    EndpointUrl, Family, GrantId, Isolation, Need,
};
use porter_fake::{
    FixedClock, MemoryStore, NOW, RecordingAudit, Scripted, ScriptedSheets, fake_service,
};

fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("name"),
        isolation: Isolation::Flatpak,
    }
}

fn files() -> Need {
    Need::Storage(StorageNeed::new(
        Access::ReadWrite,
        Delta::Poll,
        StorageScope::AppFolder,
        QuotaReport::Unreported,
    ))
}

fn mail() -> Need {
    Need::Mail(MailNeed::new(Access::Read, Offered::Present, Delta::Poll))
}

fn url(text: &str) -> EndpointUrl {
    EndpointUrl::parse(text).expect("url")
}

async fn choose(service: &impl Serves, app: &AppId, need: Need, class: DataClass) -> AccountsReply {
    service
        .serve(
            app,
            AccountsRequest::Choose {
                need,
                class,
                usage: Usage::Interactive,
                window: ParentWindow::Unparented,
            },
        )
        .await
}

/// The service under test, as far as these tests drive it.
trait Serves {
    fn serve(
        &self,
        app: &AppId,
        request: AccountsRequest,
    ) -> impl std::future::Future<Output = AccountsReply> + Send;
}

impl<P, S, U, R, A> Serves for porter_service::AccountService<P, S, U, FixedClock, R, A>
where
    P: porter_provider::Provider,
    S: porter_secrets::Secrets,
    U: porter_service::Sheets,
    R: porter_service::RegistryStore,
    A: porter_service::AuditSink,
{
    async fn serve(&self, app: &AppId, request: AccountsRequest) -> AccountsReply {
        self.handle(app, request).await
    }
}

fn granted(reply: AccountsReply) -> porter_core::Candidate {
    match reply {
        AccountsReply::Chosen(candidate) => candidate,
        other => panic!("not chosen: {other:?}"),
    }
}

#[tokio::test]
async fn a_grant_covers_the_audiences_of_its_own_kind_only() {
    let sheets = ScriptedSheets::answering([
        Scripted::AllowFirst(GrantScope::Always),
        Scripted::AllowFirst(GrantScope::Always),
    ]);
    let service = fake_service(sheets).await;
    let me = app("org.quire.Mail");
    let storage = granted(choose(&service, &me, files(), DataClass::Files).await);
    let sending = granted(choose(&service, &me, mail(), DataClass::Mail).await);
    let token = |grant: &GrantId, audience: &str| {
        service.handle(
            &me,
            AccountsRequest::IssueToken {
                grant: grant.clone(),
                audience: Audience(audience.into()),
            },
        )
    };
    assert!(matches!(
        token(&storage.grant, "webdav").await,
        AccountsReply::Token(_)
    ));
    assert_eq!(
        token(&storage.grant, "imap").await,
        AccountsReply::Refused(Refusal::AudienceNotGranted)
    );
    assert!(matches!(
        token(&sending.grant, "imap").await,
        AccountsReply::Token(_)
    ));
    assert!(matches!(
        token(&sending.grant, "smtp").await,
        AccountsReply::Token(_)
    ));
    assert_eq!(
        token(&sending.grant, "webdav").await,
        AccountsReply::Refused(Refusal::AudienceNotGranted)
    );
}

#[tokio::test]
async fn a_candidate_lists_the_endpoints_of_the_kind_that_fit() {
    let sheets = ScriptedSheets::answering([
        Scripted::AllowFirst(GrantScope::Always),
        Scripted::AllowFirst(GrantScope::Always),
    ]);
    let service = fake_service(sheets).await;
    let me = app("org.quire.Mail");
    let storage = granted(choose(&service, &me, files(), DataClass::Files).await);
    let sending = granted(choose(&service, &me, mail(), DataClass::Mail).await);
    let families = |candidate: &porter_core::Candidate| -> Vec<Family> {
        candidate.endpoints.iter().map(|e| e.family).collect()
    };
    assert_eq!(
        families(&storage),
        vec![Family::WebDav],
        "no CalDAV root for files"
    );
    assert_eq!(families(&sending), vec![Family::Imap, Family::Smtp]);
    let queried = service
        .handle(
            &me,
            AccountsRequest::Query {
                need: mail(),
                class: DataClass::Mail,
                usage: Usage::Interactive,
            },
        )
        .await;
    assert_eq!(queried, AccountsReply::Candidates(vec![sending]));
}

#[tokio::test]
async fn a_relay_may_dial_only_an_endpoint_the_account_holds_for_the_grants_kind() {
    let sheets = ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always)]);
    let service = fake_service(sheets).await;
    let me = app("org.quire.Photos");
    let storage = granted(choose(&service, &me, files(), DataClass::Files).await);
    let open = |who: &AppId, grant: &GrantId, endpoint: &str| {
        let endpoint = url(endpoint);
        let who = who.clone();
        let grant = grant.clone();
        let service = &service;
        async move { service.open_authenticated(&who, &grant, &endpoint).await }
    };
    let own = "https://cloud.invalid/remote.php/dav/files/ada/";
    const REFUSED: &[(&str, &str, Refusal)] = &[
        (
            "an address the app made up",
            "https://evil.invalid/remote.php/dav/files/ada/",
            Refusal::EndpointNotGranted,
        ),
        (
            "the account's CalDAV root under a files grant",
            "https://cloud.invalid/remote.php/dav/calendars/ada/",
            Refusal::EndpointNotGranted,
        ),
        (
            "the right host on another port",
            "https://cloud.invalid:8443/remote.php/dav/files/ada/",
            Refusal::EndpointNotGranted,
        ),
    ];
    for (name, endpoint, refusal) in REFUSED {
        assert_eq!(
            open(&me, &storage.grant, endpoint).await.err(),
            Some(*refusal),
            "{name}"
        );
    }
    assert_eq!(
        open(&app("org.quire.Notes"), &storage.grant, own)
            .await
            .err(),
        Some(Refusal::UnknownGrant),
        "another app's grant"
    );
    assert_eq!(
        open(&me, &GrantId::parse("nope").expect("id"), own)
            .await
            .err(),
        Some(Refusal::UnknownGrant)
    );
}

#[tokio::test]
async fn a_relay_to_an_endpoint_the_account_holds_is_planned() {
    let sheets = ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always)]);
    let service = fake_service(sheets).await;
    let me = app("org.quire.Photos");
    let storage = granted(choose(&service, &me, files(), DataClass::Files).await);
    let plan = service
        .open_authenticated(&me, &storage.grant, &storage.endpoints[0].url)
        .await
        .expect("planned");
    assert_eq!(plan.endpoint, storage.endpoints[0]);
}

#[tokio::test]
async fn an_opened_relay_is_audited_with_its_grant_and_endpoint_and_a_refused_one_is_not() {
    let audit = RecordingAudit::default();
    let sheets = ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always)]);
    let service = fake_service(sheets).await.with_audit(audit.clone());
    let me = app("org.quire.Photos");
    let storage = granted(choose(&service, &me, files(), DataClass::Files).await);
    let opened = |entries: Vec<porter_core::audit::AuditEntry>| {
        entries
            .into_iter()
            .filter(|e| matches!(e.event, AuditEvent::ProxyOpened { .. }))
            .collect::<Vec<_>>()
    };
    let refused = service
        .open_authenticated(
            &me,
            &storage.grant,
            &url("https://evil.invalid/remote.php/dav/files/ada/"),
        )
        .await;
    assert_eq!(refused.err(), Some(Refusal::EndpointNotGranted));
    assert!(opened(audit.entries()).is_empty());

    service
        .open_authenticated(&me, &storage.grant, &storage.endpoints[0].url)
        .await
        .expect("planned");
    let entries = opened(audit.entries());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].app.as_ref(), Some(&me));
    assert_eq!(
        entries[0].event,
        AuditEvent::ProxyOpened {
            grant: storage.grant.clone(),
            endpoint: storage.endpoints[0].url.clone(),
        }
    );
}

#[tokio::test]
async fn a_linked_relay_dials_only_an_origin_the_provider_file_declares_and_presents_nothing() {
    use porter_core::RelayAuth;

    let audit = RecordingAudit::default();
    let sheets = ScriptedSheets::answering([
        Scripted::AllowFirst(GrantScope::Always),
        Scripted::AllowFirst(GrantScope::Always),
    ]);
    let service = fake_service(sheets).await.with_audit(audit.clone());
    let me = app("org.quire.Photos");
    let storage = granted(choose(&service, &me, files(), DataClass::Files).await);
    let mailer = granted(choose(&service, &me, mail(), DataClass::Mail).await);
    let opened = |entries: Vec<porter_core::audit::AuditEntry>| {
        entries
            .into_iter()
            .filter(|e| matches!(e.event, AuditEvent::ProxyOpened { .. }))
            .count()
    };

    let plan = service
        .open_linked(&me, &storage.grant, &url("https://files.cdn.cloud.invalid"))
        .await
        .expect("a declared origin");
    assert_eq!(plan.auth, RelayAuth::Anonymous);
    assert_eq!(plan.kind, CapabilityKind::Storage);
    assert_eq!(
        plan.endpoint.url,
        url("https://files.cdn.cloud.invalid:443")
    );
    assert_eq!(plan.endpoint.family, Family::WebDav);
    assert_eq!(opened(audit.entries()), 1);

    const REFUSED: &[(&str, &str)] = &[
        (
            "an origin the file does not declare",
            "https://evil.invalid",
        ),
        ("the account's own host", "https://cloud.invalid"),
        ("the suffix itself", "https://cdn.cloud.invalid"),
        (
            "a host that only ends alike",
            "https://evilcdn.cloud.invalid",
        ),
        (
            "a declared host on another port",
            "https://a.cdn.cloud.invalid:8443",
        ),
        ("a downgrade to plain http", "http://a.cdn.cloud.invalid"),
        ("a link with a path", "https://a.cdn.cloud.invalid/up/s1"),
        ("another scheme", "imaps://a.cdn.cloud.invalid"),
    ];
    for (name, origin) in REFUSED {
        assert_eq!(
            service
                .open_linked(&me, &storage.grant, &url(origin))
                .await
                .err(),
            Some(Refusal::EndpointNotGranted),
            "{name}"
        );
    }
    assert_eq!(
        service
            .open_linked(&me, &mailer.grant, &url("https://files.cdn.cloud.invalid"))
            .await
            .err(),
        Some(Refusal::EndpointNotGranted),
        "a mail grant has no linked origins"
    );
    assert_eq!(
        service
            .open_linked(
                &app("org.quire.Notes"),
                &storage.grant,
                &url("https://files.cdn.cloud.invalid")
            )
            .await
            .err(),
        Some(Refusal::UnknownGrant),
        "another app's grant"
    );
    assert_eq!(opened(audit.entries()), 1, "a refused one is not audited");
}

#[tokio::test]
async fn an_authenticated_relay_reaches_an_origin_the_file_lists_with_the_credential_and_no_other()
{
    use porter_core::RelayAuth;

    let audit = RecordingAudit::default();
    let sheets = ScriptedSheets::answering([
        Scripted::AllowFirst(GrantScope::Always),
        Scripted::AllowFirst(GrantScope::Always),
    ]);
    let service = fake_service(sheets).await.with_audit(audit.clone());
    let me = app("org.quire.Photos");
    let storage = granted(choose(&service, &me, files(), DataClass::Files).await);
    let mailer = granted(choose(&service, &me, mail(), DataClass::Mail).await);

    let plan = service
        .open_authenticated(&me, &storage.grant, &url("https://media.cloud.invalid"))
        .await
        .expect("a listed origin");
    assert_eq!(plan.endpoint.url, url("https://media.cloud.invalid:443"));
    assert_eq!(plan.endpoint.family, Family::WebDav);
    match &plan.auth {
        RelayAuth::Password(password) => assert_eq!(password.expose(), "app-pw"),
        other => panic!("the credential goes with it, got {other:?}"),
    }

    const REFUSED: &[(&str, &str)] = &[
        ("an origin the file does not list", "https://evil.invalid"),
        (
            "a host under the listed one",
            "https://a.media.cloud.invalid",
        ),
        (
            "a listed host on another port",
            "https://media.cloud.invalid:8443",
        ),
        ("a downgrade to plain http", "http://media.cloud.invalid"),
        ("a path on a listed host", "https://media.cloud.invalid/x"),
        (
            "a linked origin, which is credential-free",
            "https://a.cdn.cloud.invalid",
        ),
    ];
    for (name, origin) in REFUSED {
        assert_eq!(
            service
                .open_authenticated(&me, &storage.grant, &url(origin))
                .await
                .err(),
            Some(Refusal::EndpointNotGranted),
            "{name}"
        );
    }
    assert_eq!(
        service
            .open_authenticated(&me, &mailer.grant, &url("https://media.cloud.invalid"))
            .await
            .err(),
        Some(Refusal::EndpointNotGranted),
        "a mail grant has none"
    );
}

#[tokio::test]
async fn a_password_account_plans_its_password_and_an_oauth_account_a_minted_token() {
    use porter_core::RelayAuth;

    let sheets = ScriptedSheets::answering([
        Scripted::AllowFirst(GrantScope::Always),
        Scripted::AllowFirst(GrantScope::Always),
    ]);
    let service = fake_service(sheets).await;
    let me = app("org.quire.Photos");
    let storage = granted(choose(&service, &me, files(), DataClass::Files).await);
    let plan = service
        .open_authenticated(&me, &storage.grant, &storage.endpoints[0].url)
        .await
        .expect("planned");
    assert_eq!(plan.kind, CapabilityKind::Storage);
    match &plan.auth {
        RelayAuth::Password(password) => assert_eq!(password.expose(), "app-pw"),
        other => panic!("expected the password, got {other:?}"),
    }

    let mailer = granted(choose(&service, &me, mail(), DataClass::Mail).await);
    for (endpoint, audience) in [
        (&mailer.endpoints[0], "imap"),
        (&mailer.endpoints[1], "smtp"),
    ] {
        let plan = service
            .open_authenticated(&me, &mailer.grant, &endpoint.url)
            .await
            .expect("planned");
        match &plan.auth {
            RelayAuth::AccessToken(token) => {
                assert_eq!(token.expose(), format!("fake:fake-mail:{audience}"));
            }
            other => panic!("expected a minted token, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn every_change_to_the_registry_is_saved_and_audited_without_a_value() {
    let store = MemoryStore::default();
    let audit = RecordingAudit::default();
    let sheets =
        ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always), Scripted::Deny]);
    let service = fake_service(sheets)
        .await
        .with_store(store.clone())
        .with_audit(audit.clone());
    let me = app("org.quire.Photos");
    let storage = granted(choose(&service, &me, files(), DataClass::Files).await);
    assert_eq!(store.saves(), 1);
    assert_eq!(store.stored().expect("saved").grants.len(), 1);
    assert_eq!(
        choose(&service, &me, mail(), DataClass::Mail).await,
        AccountsReply::Refused(Refusal::Denied)
    );
    let token = service
        .handle(
            &me,
            AccountsRequest::IssueToken {
                grant: storage.grant.clone(),
                audience: Audience("webdav".into()),
            },
        )
        .await;
    assert!(matches!(token, AccountsReply::Token(_)));
    assert_eq!(
        service
            .handle(
                &me,
                AccountsRequest::Revoke {
                    grant: storage.grant.clone()
                }
            )
            .await,
        AccountsReply::Revoked
    );
    let removed: AccountId = storage.account.clone();
    service.remove_account(&removed).await.expect("removed");
    let saved = store.stored().expect("saved");
    assert!(saved.accounts.iter().all(|a| a.id != removed));
    assert!(saved.grants.iter().all(|g| g.key.account != removed));

    let events: Vec<_> = audit
        .entries()
        .into_iter()
        .map(|e| (e.at, e.event))
        .collect();
    assert_eq!(
        events,
        vec![
            (
                NOW,
                AuditEvent::Granted {
                    grant: storage.grant.clone(),
                    kind: CapabilityKind::Storage
                }
            ),
            (
                NOW,
                AuditEvent::Denied {
                    kind: CapabilityKind::Mail
                }
            ),
            (
                NOW,
                AuditEvent::TokenIssued {
                    grant: storage.grant.clone(),
                    audience: Audience("webdav".into())
                }
            ),
            (
                NOW,
                AuditEvent::Revoked {
                    grant: storage.grant.clone()
                }
            ),
            (NOW, AuditEvent::Removed),
        ]
    );
    let shown = format!("{:?}", audit.entries());
    for secret in ["app-pw", "fake:fake-storage"] {
        assert!(!shown.contains(secret), "{secret} in the audit log");
    }
}

#[tokio::test]
async fn a_registry_that_cannot_be_saved_is_reported_unavailable() {
    let store = MemoryStore::default();
    store.refusing(true);
    let sheets = ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always)]);
    let service = fake_service(sheets).await.with_store(store.clone());
    let me = app("org.quire.Photos");
    assert_eq!(
        choose(&service, &me, files(), DataClass::Files).await,
        AccountsReply::Refused(Refusal::Unavailable)
    );
    assert_eq!(store.saves(), 0);
}

#[tokio::test]
async fn a_service_starts_from_what_the_store_holds() {
    let first = MemoryStore::default();
    let sheets = ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always)]);
    let service = fake_service(sheets).await.with_store(first.clone());
    let me = app("org.quire.Photos");
    let storage = granted(choose(&service, &me, files(), DataClass::Files).await);
    let stored = first.stored().expect("saved");
    let registry = porter_service::Registry::from_persisted(stored.clone());
    assert_eq!(registry.persisted(), stored);
    assert_eq!(registry.grants[0].id, storage.grant);
}
