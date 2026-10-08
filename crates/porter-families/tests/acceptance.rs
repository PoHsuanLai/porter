//! Acceptance 1 of design/31 §7.1, in process: a Nextcloud account is added by Login Flow v2
//! against the fake; a second app's `Query(Storage)` finds nothing until consent is accepted, and
//! then one candidate with the account's endpoints. Around it, the rest of what the service does
//! with a real family: sign in again, close the sheet mid-wait, a registry that cannot be
//! written, a service turned off, add-and-allow.
#![cfg(all(feature = "nextcloud", feature = "generic"))]

mod common;

use common::{Brief, Fakes, Kept, Person, Service, TestSheets, plain, service};
use porter_core::audit::AuditEvent;
use porter_core::capability::{Access, CapabilityKind, Delta, QuotaReport, StorageScope};
use porter_core::consent::{GrantScope, Usage};
use porter_core::need::StorageNeed;
use porter_core::sheet::{FieldKind, SheetView};
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{
    AbsentReason, AccountId, AccountState, AccountsReply, AccountsRequest, AppId, AppName,
    Audience, DataClass, Family, Isolation, Need, Offer, ProviderId,
};
use porter_fake::{Scripted, ScriptedSheets};
use porter_fake_servers::{FakeNextcloud, LoginPolicy, NextcloudHandle, Running, shipped};
use porter_families::{FamilyProvider, NextcloudProvider, Pacing};
use porter_http::SharedHttp;
use porter_service::AllowFor;
use std::time::Duration;

fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("app name"),
        isolation: Isolation::Flatpak,
    }
}

fn storage() -> Need {
    Need::Storage(StorageNeed {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        scope: StorageScope::Full,
        quota: QuotaReport::Reported,
    })
}

fn query() -> AccountsRequest {
    AccountsRequest::Query {
        need: storage(),
        class: DataClass::Files,
        usage: Usage::Interactive,
    }
}

fn provider() -> FamilyProvider {
    FamilyProvider::Nextcloud(
        NextcloudProvider::new(
            shipped::nextcloud(),
            SharedHttp::new(Fakes::loopback()),
            Brief,
        )
        .with_pacing(Pacing {
            first: Duration::from_secs(1),
            step: Duration::from_secs(1),
            limit: Duration::from_secs(100_000),
        }),
    )
}

/// Someone adding a Nextcloud: types its address and, when the page is shown, opens it.
fn adding(server: &str) -> Person {
    Person {
        pick: Some(ProviderId::parse("nextcloud").expect("id")),
        answers: vec![plain(FieldKind::Server, server)],
        opens_page_after: Some(Duration::ZERO),
        ..Person::default()
    }
}

async fn nextcloud() -> Running<NextcloudHandle> {
    let running = FakeNextcloud::start("alice").await.expect("nextcloud");
    running.set_login_policy(LoginPolicy::Approve);
    running
}

/// The id the account of "alice" at the fake gets: the provider and the label (login, host and
/// the fake's port) as a slug.
fn expected_id(nextcloud: &Running<NextcloudHandle>, suffix: &str) -> String {
    let port = nextcloud.base_url().rsplit(':').next().expect("port");
    format!("nextcloud-alice-127.0.0.1-{port}{suffix}")
}

fn added(reply: AccountsReply) -> AccountId {
    match reply {
        AccountsReply::Added(id) => id,
        other => panic!("expected Added, got {other:?}"),
    }
}

fn consent(answers: Vec<Scripted>) -> ScriptedSheets {
    ScriptedSheets::answering(answers)
}

async fn with_account(
    nextcloud: &Running<NextcloudHandle>,
    consent: ScriptedSheets,
    people: Vec<Person>,
) -> (Service, Kept, AccountId) {
    let (service, kept) = service(vec![provider()], TestSheets::new(people, consent));
    let id = added(
        service
            .handle(
                &app("org.quire.Photos"),
                AccountsRequest::AddAccount {
                    hint: ProviderHint::Any,
                    window: ParentWindow::Unparented,
                },
            )
            .await,
    );
    let _ = nextcloud;
    (service, kept, id)
}

#[tokio::test]
async fn adding_a_nextcloud_account_by_login_flow_then_a_second_app_asks_and_is_granted() {
    let nextcloud = nextcloud().await;
    let (service, kept, id) = with_account(
        &nextcloud,
        consent(vec![Scripted::AllowFirst(GrantScope::Always)]),
        vec![adding(nextcloud.base_url())],
    )
    .await;
    assert_eq!(id.as_str(), expected_id(&nextcloud, ""));

    // The account is stored, working, with its servers; the app password is filed and the
    // registry was written.
    let registry = service.registry();
    let [account] = registry.accounts.as_slice() else {
        panic!("one account: {:?}", registry.accounts);
    };
    assert_eq!(account.state, AccountState::Ok);
    assert_eq!(account.provider.as_str(), "nextcloud");
    assert_eq!(account.endpoints.len(), 4);
    assert!(
        registry.grants.is_empty(),
        "adding grants nothing to anyone"
    );
    assert_eq!(kept.store.stored().expect("saved").accounts.len(), 1);
    let password = kept
        .secrets
        .password(&id)
        .await
        .expect("the app password is filed");
    assert!(nextcloud.app_passwords().contains(&password));

    // A second app: nothing until it is allowed.
    let files = app("org.quire.Files");
    assert_eq!(
        service.handle(&files, query()).await,
        AccountsReply::Candidates(vec![])
    );
    let reply = service
        .handle(
            &files,
            AccountsRequest::Choose {
                need: storage(),
                class: DataClass::Files,
                usage: Usage::Interactive,
                window: ParentWindow::Unparented,
            },
        )
        .await;
    let AccountsReply::Chosen(chosen) = reply else {
        panic!("consent was accepted: {reply:?}");
    };
    assert_eq!(chosen.account, id);

    let AccountsReply::Candidates(found) = service.handle(&files, query()).await else {
        panic!("candidates");
    };
    let [candidate] = found.as_slice() else {
        panic!("one candidate: {found:?}");
    };
    let endpoints: Vec<(Family, String)> = candidate
        .endpoints
        .iter()
        .map(|e| (e.family, e.login.0.clone()))
        .collect();
    assert_eq!(endpoints, [(Family::WebDav, "alice".to_owned())]);
    assert!(
        candidate.endpoints[0]
            .url
            .as_str()
            .ends_with("/remote.php/dav/files/alice/")
    );
    assert_eq!(candidate.grant, chosen.grant);

    // The password is not in anything an app is told, and no token carries it.
    assert!(!format!("{found:?}").contains(&password));
    let token = service
        .handle(
            &files,
            AccountsRequest::IssueToken {
                grant: chosen.grant.clone(),
                audience: Audience("webdav".into()),
            },
        )
        .await;
    assert_eq!(token, AccountsReply::Refused(Refusal::Denied));

    // The audit log names the sign-in (the first app) and the grant (the second), no values.
    let entries = kept.audit.entries();
    assert!(
        entries
            .iter()
            .any(|e| e.event == AuditEvent::SignedIn && e.app == Some(app("org.quire.Photos"))),
        "{entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|e| matches!(e.event, AuditEvent::Granted { .. }) && e.app == Some(files.clone()))
    );
    assert!(!format!("{entries:?}").contains(&password));
}

#[tokio::test]
async fn a_hint_skips_the_list_and_the_sheet_walks_the_sign_in() {
    let nextcloud = nextcloud().await;
    let sheets = TestSheets::new(vec![adding(nextcloud.base_url())], consent(vec![]));
    let shown = sheets.shown();
    let (service, _kept) = service(vec![provider()], sheets);
    let reply = service
        .handle(
            &app("org.quire.Photos"),
            AccountsRequest::AddAccount {
                hint: ProviderHint::Provider(ProviderId::parse("nextcloud").expect("id")),
                window: ParentWindow::Unparented,
            },
        )
        .await;
    added(reply);
    let views: Vec<&str> = shown
        .lock()
        .expect("shown")
        .iter()
        .map(|v| match v {
            SheetView::Providers(_) => "providers",
            SheetView::Working { .. } => "working",
            SheetView::SignIn(_) => "form",
            SheetView::BrowserWait { .. } => "browser",
            SheetView::Review(_) => "review",
            SheetView::Done => "done",
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    // Working first (the provider is known), a form for the server, the page, the review, done;
    // each `Working` is the sign-in thinking between them.
    assert_eq!(views.first(), Some(&"working"));
    assert!(!views.contains(&"providers"), "{views:?}");
    let order: Vec<&&str> = views.iter().filter(|v| **v != "working").collect();
    assert_eq!(order, [&"form", &"browser", &"review", &"done"]);
}

#[tokio::test]
async fn a_hint_for_a_provider_that_is_not_installed_is_unavailable() {
    let (service, _kept) = service(vec![provider()], TestSheets::new(vec![], consent(vec![])));
    let reply = service
        .handle(
            &app("org.quire.Photos"),
            AccountsRequest::AddAccount {
                hint: ProviderHint::Provider(ProviderId::parse("nope").expect("id")),
                window: ParentWindow::Unparented,
            },
        )
        .await;
    assert_eq!(reply, AccountsReply::Refused(Refusal::Unavailable));
}

#[tokio::test]
async fn the_person_may_take_their_time_in_the_browser() {
    let nextcloud = nextcloud().await;
    let mut person = adding(nextcloud.base_url());
    person.opens_page_after = Some(Duration::from_millis(60));
    let (_service, _kept, id) = with_account(&nextcloud, consent(vec![]), vec![person]).await;
    assert_eq!(id.as_str(), expected_id(&nextcloud, ""));
    let polls = nextcloud
        .hits()
        .into_iter()
        .filter(|h| h.target == "/index.php/login/v2/poll")
        .count();
    assert!(polls > 1, "it waited: {polls} polls");
}

#[tokio::test]
async fn closing_the_sheet_while_waiting_for_the_browser_adds_nothing() {
    let nextcloud = nextcloud().await;
    nextcloud.set_login_policy(LoginPolicy::Pending);
    let mut person = adding(nextcloud.base_url());
    person.opens_page_after = None;
    person.gives_up = true;
    let (service, kept) = service(
        vec![provider()],
        TestSheets::new(vec![person], consent(vec![])),
    );
    let reply = service
        .handle(
            &app("org.quire.Photos"),
            AccountsRequest::AddAccount {
                hint: ProviderHint::Any,
                window: ParentWindow::Unparented,
            },
        )
        .await;
    assert_eq!(reply, AccountsReply::Refused(Refusal::Dismissed));
    assert!(service.registry().accounts.is_empty());
    assert_eq!(kept.store.saves(), 0);
    assert_eq!(nextcloud.app_passwords(), Vec::<String>::new());
    assert_eq!(kept.audit.entries(), vec![]);
}

#[tokio::test]
async fn a_registry_that_cannot_be_written_leaves_no_account_and_no_secret() {
    let nextcloud = nextcloud().await;
    let (service, kept) = service(
        vec![provider()],
        TestSheets::new(vec![adding(nextcloud.base_url())], consent(vec![])),
    );
    kept.store.refusing(true);
    let reply = service
        .handle(
            &app("org.quire.Photos"),
            AccountsRequest::AddAccount {
                hint: ProviderHint::Any,
                window: ParentWindow::Unparented,
            },
        )
        .await;
    assert_eq!(reply, AccountsReply::Refused(Refusal::Unavailable));
    assert!(service.registry().accounts.is_empty());
    assert_eq!(
        kept.secrets
            .password(&AccountId::parse(&expected_id(&nextcloud, "")).expect("id"))
            .await,
        None
    );
    assert_eq!(
        kept.audit.entries(),
        vec![],
        "a sign-in that was not stored is not logged"
    );
}

#[tokio::test]
async fn a_service_the_person_turns_off_is_absent_and_remembered() {
    let nextcloud = nextcloud().await;
    let mut person = adding(nextcloud.base_url());
    person.off = vec![CapabilityKind::Notes];
    let (service, kept, id) = with_account(&nextcloud, consent(vec![]), vec![person]).await;
    let registry = service.registry();
    let account = &registry.accounts[0];
    let notes = account
        .capabilities
        .iter()
        .find(|c| c.offer.kind() == CapabilityKind::Notes)
        .expect("a notes claim");
    assert!(matches!(
        notes.offer,
        Offer::Absent {
            reason: AbsentReason::TurnedOff,
            ..
        }
    ));
    assert_eq!(registry.toggles.len(), 1);
    assert_eq!(registry.toggles[0].account, id);
    assert_eq!(kept.store.stored().expect("saved").toggles.len(), 1);
}

#[tokio::test]
async fn adding_the_same_account_twice_makes_two_accounts_with_their_own_ids() {
    let nextcloud = nextcloud().await;
    let (service, _kept) = service(
        vec![provider()],
        TestSheets::new(
            vec![adding(nextcloud.base_url()), adding(nextcloud.base_url())],
            consent(vec![]),
        ),
    );
    let mut ids = Vec::new();
    for _ in 0..2 {
        ids.push(added(
            service
                .handle(
                    &app("org.quire.Photos"),
                    AccountsRequest::AddAccount {
                        hint: ProviderHint::Any,
                        window: ParentWindow::Unparented,
                    },
                )
                .await,
        ));
    }
    assert_eq!(
        ids.iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>(),
        [expected_id(&nextcloud, ""), expected_id(&nextcloud, "-2")]
    );
}

#[tokio::test]
async fn signing_in_again_replaces_the_password_for_an_app_that_holds_a_grant() {
    let nextcloud = nextcloud().await;
    let (service, kept, id) = with_account(
        &nextcloud,
        consent(vec![Scripted::AllowFirst(GrantScope::Always)]),
        vec![adding(nextcloud.base_url()), adding(nextcloud.base_url())],
    )
    .await;
    let files = app("org.quire.Files");
    let AccountsReply::Chosen(_) = service
        .handle(
            &files,
            AccountsRequest::Choose {
                need: storage(),
                class: DataClass::Files,
                usage: Usage::Interactive,
                window: ParentWindow::Unparented,
            },
        )
        .await
    else {
        panic!("granted");
    };
    let before = kept.secrets.password(&id).await.expect("filed");

    // An app with no grant for the account cannot even ask; neither can anyone for no account.
    let reauth = |account: &AccountId| AccountsRequest::Reauthenticate {
        account: account.clone(),
        window: ParentWindow::Unparented,
    };
    assert_eq!(
        service.handle(&app("org.quire.Other"), reauth(&id)).await,
        AccountsReply::Refused(Refusal::UnknownGrant)
    );
    assert_eq!(
        service
            .handle(&files, reauth(&AccountId::parse("nope").expect("id")))
            .await,
        AccountsReply::Refused(Refusal::UnknownGrant)
    );

    assert_eq!(
        service.handle(&files, reauth(&id)).await,
        AccountsReply::Reauthenticated
    );
    let after = kept.secrets.password(&id).await.expect("filed again");
    assert_ne!(before, after, "a new app password replaced the old one");
    assert!(nextcloud.app_passwords().contains(&after));
    assert_eq!(
        service.registry().accounts.len(),
        1,
        "the same account, not a second one"
    );
    assert_eq!(service.registry().accounts[0].state, AccountState::Ok);
    let entries = kept.audit.entries();
    assert!(
        entries.iter().any(|e| e.event == AuditEvent::Reauthed
            && e.account.as_ref() == Some(&id)
            && e.app == Some(files.clone())),
        "{entries:?}"
    );
}

#[tokio::test]
async fn add_and_allow_is_one_step_with_one_grant() {
    let nextcloud = nextcloud().await;
    let (service, kept) = service(
        vec![provider()],
        TestSheets::new(vec![adding(nextcloud.base_url())], consent(vec![])),
    );
    let files = app("org.quire.Files");
    let reply = service
        .add_and_allow(
            &files,
            ProviderHint::Any,
            ParentWindow::Unparented,
            AllowFor {
                need: storage(),
                class: DataClass::Files,
                usage: Usage::Interactive,
            },
        )
        .await;
    let id = added(reply);
    let grants = service.registry().grants;
    assert_eq!(grants.len(), 1);
    assert_eq!((&grants[0].key.app, &grants[0].key.account), (&files, &id));
    // No second prompt: the app finds the account at once.
    let AccountsReply::Candidates(found) = service.handle(&files, query()).await else {
        panic!("candidates");
    };
    assert_eq!(found.len(), 1);
    let kinds: Vec<_> = kept
        .audit
        .entries()
        .iter()
        .map(|e| std::mem::discriminant(&e.event))
        .collect();
    assert_eq!(kinds.len(), 2, "SignedIn and Granted");
}

#[tokio::test]
async fn add_and_allow_grants_nothing_when_the_new_account_does_not_meet_the_need() {
    let nextcloud = nextcloud().await;
    let (service, _kept) = service(
        vec![provider()],
        TestSheets::new(vec![adding(nextcloud.base_url())], consent(vec![])),
    );
    let mail = Need::Mail(porter_core::need::MailNeed {
        access: Access::ReadWrite,
        send: porter_core::capability::Offered::Present,
        delta: Delta::Poll,
    });
    let reply = service
        .add_and_allow(
            &app("org.quire.Mail"),
            ProviderHint::Any,
            ParentWindow::Unparented,
            AllowFor {
                need: mail,
                class: DataClass::Mail,
                usage: Usage::Interactive,
            },
        )
        .await;
    added(reply);
    assert!(service.registry().grants.is_empty());
}

#[tokio::test]
async fn removing_a_nextcloud_account_deletes_its_app_password_at_the_server_then_wipes_it() {
    let nextcloud = nextcloud().await;
    let (service, kept, id) = with_account(
        &nextcloud,
        consent(vec![]),
        vec![adding(nextcloud.base_url())],
    )
    .await;
    let password = kept.secrets.password(&id).await.expect("filed");
    assert!(nextcloud.app_passwords().contains(&password));

    service.remove_account(&id).await.expect("removed");

    assert!(
        !nextcloud.app_passwords().contains(&password),
        "the server no longer honours the app password"
    );
    assert!(service.registry().accounts.is_empty());
    assert_eq!(kept.secrets.password(&id).await, None);
}

fn choose_as(files: &AppId) -> (AppId, AccountsRequest) {
    (
        files.clone(),
        AccountsRequest::Choose {
            need: storage(),
            class: DataClass::Files,
            usage: Usage::Interactive,
            window: ParentWindow::Unparented,
        },
    )
}

#[tokio::test]
async fn add_account_on_the_alert_adds_and_allows_in_one_step_with_one_grant() {
    let nextcloud = nextcloud().await;
    let (service, kept, first) = with_account(
        &nextcloud,
        consent(vec![Scripted::AddAccount]),
        vec![adding(nextcloud.base_url()), adding(nextcloud.base_url())],
    )
    .await;
    let (files, request) = choose_as(&app("org.quire.Files"));
    let AccountsReply::Chosen(chosen) = service.handle(&files, request).await else {
        panic!("the new account is chosen");
    };
    assert_ne!(chosen.account, first, "the account is the one just added");
    let registry = service.registry();
    assert_eq!(registry.accounts.len(), 2);
    assert_eq!(registry.grants.len(), 1, "one grant, not a second prompt");
    let grant = &registry.grants[0];
    assert_eq!(
        (&grant.key.app, &grant.key.account),
        (&files, &chosen.account)
    );
    assert_eq!(grant.id, chosen.grant);
    let granted = kept
        .audit
        .entries()
        .iter()
        .filter(|e| matches!(e.event, AuditEvent::Granted { .. }))
        .count();
    assert_eq!(granted, 1);
}

#[tokio::test]
async fn cancelling_the_add_from_the_alert_grants_nothing() {
    let nextcloud = nextcloud().await;
    let mut gives_up = adding(nextcloud.base_url());
    gives_up.opens_page_after = None;
    gives_up.gives_up = true;
    let (service, _kept, _first) = with_account(
        &nextcloud,
        consent(vec![Scripted::AddAccount]),
        vec![adding(nextcloud.base_url()), gives_up],
    )
    .await;
    nextcloud.set_login_policy(LoginPolicy::Pending);
    let (files, request) = choose_as(&app("org.quire.Files"));
    let reply = service.handle(&files, request).await;
    assert!(
        matches!(reply, AccountsReply::Refused(_)),
        "no account chosen: {reply:?}"
    );
    let registry = service.registry();
    assert_eq!(registry.accounts.len(), 1);
    assert!(
        registry.grants.is_empty(),
        "cancelling the add grants nothing"
    );
}
