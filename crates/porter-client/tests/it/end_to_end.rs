//! An app on the in-process transport, over the fake providers: it finds an account by
//! capability, is asked for consent, and gets a token; a refusal sticks; a once-grant is spent.
#![cfg(feature = "in-process")]

use porter_client::{Accounts, ClientError, ConsentOffer, Found, InProcess, NoAccount};
use porter_core::capability::{Access, Delta, QuotaReport, StorageScope};
use porter_core::consent::{GrantScope, Usage};
use porter_core::need::StorageNeed;
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{AccountId, AppId, AppName, Audience, DataClass, Isolation, Need};
use porter_fake::{FakeService, Scripted, ScriptedSheets, fake_service};
use std::sync::Arc;

type App = Accounts<InProcess<FakeService>>;

fn app(service: &Arc<FakeService>, name: &str) -> App {
    let id = AppId {
        name: AppName::parse(name).expect("app name"),
        isolation: Isolation::Flatpak,
    };
    Accounts::over(InProcess::new(Arc::clone(service), id))
}

fn storage(delta: Delta) -> Need {
    Need::Storage(StorageNeed::new(
        Access::ReadWrite,
        delta,
        StorageScope::AppFolder,
        QuotaReport::Unreported,
    ))
}

fn needs_consent(found: Found) -> ConsentOffer {
    match found {
        Found::NeedsConsent(offer) => offer,
        other => panic!("expected NeedsConsent, got {other:?}"),
    }
}

#[tokio::test]
async fn a_storage_need_finds_the_fake_cloud_after_consent() {
    let prompter = ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always)]);
    let log = prompter.log();
    let service = Arc::new(fake_service(prompter).await);
    let photos = app(&service, "org.quire.Photos");
    let need = storage(Delta::Poll);

    let offer = needs_consent(
        photos
            .find(&need, DataClass::Photos, Usage::Interactive)
            .await
            .expect("find"),
    );
    let chosen = photos
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");

    let storage_id = AccountId::parse("fake-storage").expect("id");
    assert_eq!(chosen.account, storage_id);
    let offered: Vec<Vec<AccountId>> = log
        .asked()
        .iter()
        .map(|ask| ask.accounts.iter().map(|c| c.account.clone()).collect())
        .collect();
    assert_eq!(
        offered,
        vec![vec![storage_id.clone()]],
        "only the storage account is offered"
    );

    match photos
        .find(&need, DataClass::Photos, Usage::Interactive)
        .await
        .expect("find")
    {
        Found::One(candidate) => assert_eq!(candidate, chosen),
        other => panic!("expected One, got {other:?}"),
    }
    let token = photos
        .token(&chosen, &Audience("webdav".into()))
        .await
        .expect("token");
    assert_eq!(token.value.expose(), "fake:fake-storage:webdav");

    let files = app(&service, "org.quire.Files");
    let other = files
        .find(&need, DataClass::Photos, Usage::Interactive)
        .await
        .expect("find");
    assert!(
        matches!(other, Found::NeedsConsent(_)),
        "another app's grant is not shared: {other:?}"
    );
}

#[tokio::test]
async fn a_refusal_sticks_for_that_app() {
    let service = Arc::new(fake_service(ScriptedSheets::answering([Scripted::Deny])).await);
    let photos = app(&service, "org.quire.Photos");
    let need = storage(Delta::Poll);
    let offer = needs_consent(
        photos
            .find(&need, DataClass::Photos, Usage::Interactive)
            .await
            .expect("find"),
    );
    let refused = photos
        .request_grant(&offer, &ParentWindow::Unparented)
        .await;
    assert_eq!(refused, Err(ClientError::Refused(Refusal::Denied)));
    let after = photos
        .find(&need, DataClass::Photos, Usage::Interactive)
        .await
        .expect("find");
    assert_eq!(after, Found::None(NoAccount::Denied));
}

#[tokio::test]
async fn a_need_no_provider_meets_is_unsupported() {
    let service = Arc::new(fake_service(ScriptedSheets::default()).await);
    let photos = app(&service, "org.quire.Photos");
    let found = photos
        .find(&storage(Delta::Push), DataClass::Photos, Usage::Interactive)
        .await
        .expect("find");
    assert_eq!(found, Found::None(NoAccount::Unsupported));
}

#[tokio::test]
async fn a_once_grant_is_spent_by_its_token() {
    let service = Arc::new(
        fake_service(ScriptedSheets::answering([Scripted::AllowFirst(
            GrantScope::Once,
        )]))
        .await,
    );
    let photos = app(&service, "org.quire.Photos");
    let need = storage(Delta::Poll);
    let offer = needs_consent(
        photos
            .find(&need, DataClass::Files, Usage::Interactive)
            .await
            .expect("find"),
    );
    let chosen = photos
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");
    photos
        .token(&chosen, &Audience("webdav".into()))
        .await
        .expect("first token");
    let again = photos.token(&chosen, &Audience("webdav".into())).await;
    assert_eq!(again, Err(ClientError::Refused(Refusal::UnknownGrant)));
    let after = photos
        .find(&need, DataClass::Files, Usage::Interactive)
        .await
        .expect("find");
    assert!(matches!(after, Found::NeedsConsent(_)), "{after:?}");
}

#[tokio::test]
async fn revoking_a_grant_ends_it() {
    let service = Arc::new(
        fake_service(ScriptedSheets::answering([Scripted::AllowFirst(
            GrantScope::Always,
        )]))
        .await,
    );
    let photos = app(&service, "org.quire.Photos");
    let need = storage(Delta::Poll);
    let offer = needs_consent(
        photos
            .find(&need, DataClass::Photos, Usage::Interactive)
            .await
            .expect("find"),
    );
    let chosen = photos
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");
    assert_eq!(photos.grants().await.expect("grants").len(), 1);
    photos.revoke(&chosen.grant).await.expect("revoke");
    assert_eq!(photos.grants().await.expect("grants"), vec![]);
}

/// A DAV account added through a real family (the generic one, over a fake DAV server) from the
/// app's own call, then granted, then signed in again: the whole path of `AddAccount` and
/// `Reauthenticate` over the in-process carrier.
#[tokio::test]
async fn an_app_adds_an_account_through_a_family_is_granted_it_and_signs_it_in_again() {
    use porter_core::capability::{Access, Delta};
    use porter_core::need::PimNeed;
    use porter_core::sheet::{FieldAnswer, FieldKind, FieldValue, ServiceChoice, SheetInput};
    use porter_core::{ProviderId, SecretText, Toggle};
    use porter_fake::{FixedClock, NOW};
    use porter_fake_servers::{FakeDav, FakeDns};
    use porter_families::{FamilyProvider, GenericProvider};
    use porter_http::{HyperHttp, SharedHttp};
    use porter_provider::parse_provider;
    use porter_secrets::MemorySecrets;
    use porter_service::{AccountService, Registry};

    let dav = FakeDav::start("bob", "hunter2").await.expect("dav");
    let spec =
        parse_provider(include_str!("../../../../providers/generic-dav.toml")).expect("file");
    let provider = FamilyProvider::Generic(GenericProvider::new(
        spec,
        SharedHttp::new(HyperHttp::new()),
        FakeDns::new(),
    ));
    let form = |password: &str| {
        SheetInput::Submit(vec![
            FieldAnswer {
                kind: FieldKind::Server,
                value: FieldValue::Plain(format!("{}/dav/calendar/", dav.base_url())),
            },
            FieldAnswer {
                kind: FieldKind::Username,
                value: FieldValue::Plain("bob".into()),
            },
            FieldAnswer {
                kind: FieldKind::Password,
                value: FieldValue::Secret(SecretText::new(password)),
            },
        ])
    };
    let review = SheetInput::Confirm(vec![
        ServiceChoice {
            kind: porter_core::CapabilityKind::Calendar,
            toggle: Toggle::On,
        },
        ServiceChoice {
            kind: porter_core::CapabilityKind::Contacts,
            toggle: Toggle::Off,
        },
    ]);
    let sheets =
        ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always)]).conversing([
            // The add: pick the provider, fill the form, confirm the review.
            vec![
                SheetInput::Pick(ProviderId::parse("generic-dav").expect("id")),
                form("hunter2"),
                review,
            ],
            // Signing in again: the form, and no review.
            vec![form("hunter2")],
            // A password the server refuses.
            vec![form("wrong"), SheetInput::Dismiss],
        ]);
    let service = Arc::new(AccountService::new(
        vec![provider],
        Registry::default(),
        MemorySecrets::default(),
        sheets,
        FixedClock(NOW),
    ));
    let calendar = app_over(&service, "org.quire.Calendar");

    let added = calendar
        .add_account(
            porter_core::wire::ProviderHint::Any,
            &ParentWindow::Unparented,
        )
        .await
        .expect("added");
    assert_eq!(added.as_str(), "generic-dav-bob-127.0.0.1");

    let need = Need::Calendar(PimNeed::new(Access::ReadWrite, Delta::Poll));
    let offer = needs_consent(
        calendar
            .find(&need, DataClass::Calendar, Usage::Interactive)
            .await
            .expect("find"),
    );
    let chosen = calendar
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");
    assert_eq!(chosen.account, added);
    assert_eq!(
        chosen.endpoints.len(),
        1,
        "only the CalDav endpoint serves a calendar"
    );

    calendar
        .reauthenticate(&added, &ParentWindow::Unparented)
        .await
        .expect("signed in again");
    assert_eq!(
        calendar
            .reauthenticate(&added, &ParentWindow::Unparented)
            .await,
        Err(ClientError::Refused(Refusal::Denied)),
        "the server refused the password"
    );
    // Another app was never granted the account, so it cannot sign it in again.
    let other = app_over(&service, "org.quire.Other");
    assert_eq!(
        other
            .reauthenticate(&added, &ParentWindow::Unparented)
            .await,
        Err(ClientError::Refused(Refusal::UnknownGrant))
    );
}

/// The mailo ask: an app adds an account that is already here (the same login at the same
/// server). The sheet says so at the review and stores nothing, and the app is told which
/// account it is, so it can go on to ask for a grant of it.
#[tokio::test]
async fn adding_an_account_that_is_already_here_tells_the_app_which_one_it_is() {
    use porter_core::sheet::{FieldAnswer, FieldKind, FieldValue, ServiceChoice, SheetInput};
    use porter_core::{ProviderId, SecretText, Toggle};
    use porter_fake::{FixedClock, NOW};
    use porter_fake_servers::{FakeDav, FakeDns};
    use porter_families::{FamilyProvider, GenericProvider};
    use porter_http::{HyperHttp, SharedHttp};
    use porter_provider::parse_provider;
    use porter_secrets::MemorySecrets;
    use porter_service::{AccountService, Registry};

    let dav = FakeDav::start("bob", "hunter2").await.expect("dav");
    let spec =
        parse_provider(include_str!("../../../../providers/generic-dav.toml")).expect("file");
    let provider = FamilyProvider::Generic(GenericProvider::new(
        spec,
        SharedHttp::new(HyperHttp::new()),
        FakeDns::new(),
    ));
    let pick = SheetInput::Pick(ProviderId::parse("generic-dav").expect("id"));
    let form = SheetInput::Submit(vec![
        FieldAnswer {
            kind: FieldKind::Server,
            value: FieldValue::Plain(format!("{}/dav/calendar/", dav.base_url())),
        },
        FieldAnswer {
            kind: FieldKind::Username,
            value: FieldValue::Plain("bob".into()),
        },
        FieldAnswer {
            kind: FieldKind::Password,
            value: FieldValue::Secret(SecretText::new("hunter2")),
        },
    ]);
    let review = SheetInput::Confirm(vec![ServiceChoice {
        kind: porter_core::CapabilityKind::Calendar,
        toggle: Toggle::On,
    }]);
    let sheets = ScriptedSheets::answering([]).conversing([
        vec![pick.clone(), form.clone(), review],
        // The same login again: the sheet stops at the review and says it is there already; the
        // person closes it.
        vec![pick, form, SheetInput::Dismiss],
    ]);
    let service = Arc::new(AccountService::new(
        vec![provider],
        Registry::default(),
        MemorySecrets::default(),
        sheets,
        FixedClock(NOW),
    ));
    let calendar = app_over(&service, "org.quire.Calendar");
    let added = calendar
        .add_account(
            porter_core::wire::ProviderHint::Any,
            &ParentWindow::Unparented,
        )
        .await
        .expect("added");
    let held = service.registry().accounts.clone();

    assert_eq!(
        calendar
            .add_account(
                porter_core::wire::ProviderHint::Any,
                &ParentWindow::Unparented,
            )
            .await,
        Err(ClientError::AlreadyAdded(added))
    );
    assert_eq!(service.registry().accounts, held, "nothing was added");
}

/// An HTTP client that finds nothing anywhere: every address publishes no server.
#[derive(Debug, Clone)]
struct NothingPublished;

impl porter_http::Http for NothingPublished {
    async fn send(
        &self,
        _request: porter_http::HttpRequest,
    ) -> Result<porter_http::HttpResponse, porter_http::HttpError> {
        Ok(porter_http::HttpResponse {
            status: porter_http::Status(404),
            headers: vec![],
            body: Vec::new(),
        })
    }
}

/// A mail address whose domain publishes no server: the sheet asks for the server by hand, and a
/// POP3 account typed there is added, then found by a mail need with its POP3 and SMTP
/// endpoints and a login name that is not the address.
#[tokio::test]
async fn a_typed_pop3_server_is_added_through_the_sheet_and_found_by_a_mail_need() {
    use porter_core::capability::Offered;
    use porter_core::need::MailNeed;
    use porter_core::sheet::{FieldAnswer, FieldKind, FieldValue, ServiceChoice, SheetInput};
    use porter_core::{Family, ProviderId, SecretText, Toggle};
    use porter_fake::{FixedClock, NOW};
    use porter_fake_servers::net::{Bind, port_of};
    use porter_fake_servers::{Accounts as MailAccounts, FakeDns, FakePop3, mailbox};
    use porter_families::{FamilyProvider, GenericProvider};
    use porter_http::SharedHttp;
    use porter_provider::parse_provider;
    use porter_secrets::MemorySecrets;
    use porter_service::{AccountService, Registry};

    // The password is tried at the POP3 server before the review, so one is running.
    let pop3 = FakePop3::start(
        &Bind::Loopback,
        porter_core::Tls::Plain,
        MailAccounts::password("ada.login", "s3cret"),
        mailbox(1),
    )
    .await
    .expect("pop3");
    let pop3_port = port_of(pop3.address());
    let plain = |kind, text: &str| FieldAnswer {
        kind,
        value: FieldValue::Plain(text.to_owned()),
    };
    let spec =
        parse_provider(include_str!("../../../../providers/generic-imap.toml")).expect("file");
    let provider = FamilyProvider::Generic(GenericProvider::new(
        spec,
        SharedHttp::new(NothingPublished),
        FakeDns::new(),
    ));
    let first = SheetInput::Submit(vec![
        plain(FieldKind::Address, "ada@old-isp.example"),
        FieldAnswer {
            kind: FieldKind::Password,
            value: FieldValue::Secret(SecretText::new("s3cret")),
        },
    ]);
    let manual = SheetInput::Submit(vec![
        plain(FieldKind::Protocol, "pop3"),
        plain(FieldKind::Server, "127.0.0.1"),
        plain(FieldKind::Security, "plain"),
        plain(FieldKind::Port, &pop3_port.to_string()),
        plain(FieldKind::OutgoingServer, "smtp.old-isp.example"),
        plain(FieldKind::OutgoingSecurity, "starttls"),
        plain(FieldKind::OutgoingPort, "2525"),
        plain(FieldKind::Username, "ada.login"),
    ]);
    let review = SheetInput::Confirm(vec![ServiceChoice {
        kind: porter_core::CapabilityKind::Mail,
        toggle: Toggle::On,
    }]);
    let sheets =
        ScriptedSheets::answering([Scripted::AllowFirst(GrantScope::Always)]).conversing([vec![
            SheetInput::Pick(ProviderId::parse("generic-imap").expect("id")),
            first,
            manual,
            review,
        ]]);
    let service = Arc::new(AccountService::new(
        vec![provider],
        Registry::default(),
        MemorySecrets::default(),
        sheets,
        FixedClock(NOW),
    ));
    let mail = app_over(&service, "org.quire.Mail");
    let added = mail
        .add_account(
            porter_core::wire::ProviderHint::Any,
            &ParentWindow::Unparented,
        )
        .await
        .expect("added");

    let need = Need::Mail(MailNeed::new(Access::Read, Offered::Present, Delta::Poll));
    let offer = needs_consent(
        mail.find(&need, DataClass::Mail, Usage::Interactive)
            .await
            .expect("find"),
    );
    let chosen = mail
        .request_grant(&offer, &ParentWindow::Unparented)
        .await
        .expect("granted");
    assert_eq!(chosen.account, added);
    let shown: Vec<_> = chosen
        .endpoints
        .iter()
        .map(|e| (e.family, e.url.to_string(), e.login.0.clone()))
        .collect();
    assert_eq!(
        shown,
        [
            (
                Family::Pop3,
                format!("pop3://127.0.0.1:{pop3_port}"),
                "ada.login".to_owned()
            ),
            (
                Family::Smtp,
                "smtp://smtp.old-isp.example:2525".to_owned(),
                "ada.login".to_owned()
            ),
        ]
    );
}

type FamilyService = porter_service::AccountService<
    porter_families::FamilyProvider,
    porter_secrets::MemorySecrets,
    ScriptedSheets,
    porter_fake::FixedClock,
>;

type FamilyApp = Accounts<InProcess<FamilyService>>;

fn app_over(service: &Arc<FamilyService>, name: &str) -> FamilyApp {
    let id = AppId {
        name: AppName::parse(name).expect("app name"),
        isolation: Isolation::Flatpak,
    };
    Accounts::over(InProcess::new(Arc::clone(service), id))
}
