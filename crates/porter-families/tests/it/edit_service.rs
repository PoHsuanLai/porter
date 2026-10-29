//! An edit of an account's servers asked of the service (`AccountsApi::edit_servers`): the
//! provider the edit names decides whether the servers are the person's to type, whether or not
//! the service holds the account; the typed servers are tried and handed back, nothing is
//! stored; a refusal marks its field and the person sends again; the password the person typed
//! comes back once and shows in no `Debug`; no bearer token is sent to a typed host.
#![cfg(feature = "generic")]

use crate::common::{Fakes, plain, secret};
use porter_core::{
    AccountId, AccountLabel, AppId, AppName, Isolation, ProviderId, SecretText, Tls,
};
use porter_fake::{
    NOW, ScriptedSheets, fake_service, mail_account, mail_provider, storage_account,
};
use porter_fake_servers::net::{Bind, port_of};
use porter_fake_servers::{
    Accounts, FakeDns, FakeImap, FakeSmtp, MailHandle, Running, Secret, mailbox,
};
use porter_families::DynProvider;
use porter_family_generic::{GenericProvider, MailLookup};
use porter_http::SharedHttp;
use porter_ids::clock::FixedClock;
use porter_provider::parse_provider;
use porter_secrets::MemorySecrets;
use porter_service::{AccountService, Service};
use porter_service_accounts::{AccountsApi, EditError};
use porter_service_core::registry::Registry;
use porter_sheet::{
    EditServers, FieldKind, Hop, MailServers, Manual, ProblemKind, Security,
    ServersFixed, SheetInput, SheetView, edit_form,
};
use porter_wire::{ParentWindow, Refusal};

const ADDRESS: &str = "ada@example.test";
const PASSWORD: &str = "pw-example-one";

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Mail").expect("app name"),
        isolation: Isolation::Flatpak,
    }
}

fn hop(port: u16) -> Hop {
    Hop {
        host: "127.0.0.1".to_owned(),
        port,
        security: Security::Plain,
    }
}

fn servers(imap: u16, smtp: u16) -> Manual {
    Manual::Imap(MailServers {
        incoming: hop(imap),
        outgoing: hop(smtp),
        login: None,
    })
}

fn request(account: &AccountId, provider: &str, current: Manual) -> EditServers {
    EditServers::new(
        account.clone(),
        ProviderId::parse(provider).expect("provider id"),
        AccountLabel(ADDRESS.to_owned()),
        current,
    )
}

fn nowhere() -> Manual {
    servers(1, 2)
}

// ---- who may edit ----

#[tokio::test]
async fn a_provider_that_signs_in_at_its_own_page_has_fixed_servers() {
    let service = fake_service(ScriptedSheets::default()).await;
    let held = mail_account().id;
    let got = service
        .accounts_api()
        .edit_servers(&app(), request(&held, "fake-mail", nowhere()), ParentWindow::Unparented)
        .await;
    assert_eq!(got.err(), Some(EditError::Fixed(ServersFixed::ProviderSignIn)));
}

#[tokio::test]
async fn a_provider_with_no_typed_servers_has_fixed_servers() {
    let service = fake_service(ScriptedSheets::default()).await;
    let got = service
        .accounts_api()
        .edit_servers(
            &app(),
            request(&AccountId::parse("fake-llm").expect("id"), "fake-llm", nowhere()),
            ParentWindow::Unparented,
        )
        .await;
    assert_eq!(got.err(), Some(EditError::Fixed(ServersFixed::NotTyped)));
}

/// The service mailo hosts holds no account: the answer cannot come from the registry.
#[tokio::test]
async fn an_oauth_provider_named_for_an_account_the_service_does_not_hold_is_fixed() {
    let service = AccountService::new(
        vec![mail_provider()],
        Registry::default(),
        MemorySecrets::default(),
        ScriptedSheets::default(),
        FixedClock(NOW),
    );
    let unheld = AccountId::parse("somewhere-else").expect("id");
    let got = service
        .accounts_api()
        .edit_servers(&app(), request(&unheld, "fake-mail", nowhere()), ParentWindow::Unparented)
        .await;
    assert_eq!(got.err(), Some(EditError::Fixed(ServersFixed::ProviderSignIn)));
}

#[tokio::test]
async fn a_provider_the_service_does_not_hold_is_refused() {
    let service = fake_service(ScriptedSheets::default()).await;
    let got = service
        .accounts_api()
        .edit_servers(
            &app(),
            request(&storage_account().id, "not-a-provider", nowhere()),
            ParentWindow::Unparented,
        )
        .await;
    assert_eq!(got.err(), Some(EditError::Refused(Refusal::Unavailable)));
}

#[tokio::test]
async fn an_account_held_under_another_provider_is_not_edited_as_the_one_named() {
    let service = fake_service(ScriptedSheets::default()).await;
    // `fake-cloud` signs in by an app password, so its servers are typed; the mail account is
    // not its account.
    let got = service
        .accounts_api()
        .edit_servers(&app(), request(&mail_account().id, "fake-cloud", nowhere()), ParentWindow::Unparented)
        .await;
    assert_eq!(got.err(), Some(EditError::NotThatProvider));
}

// ---- an edit through the generic family, over fake mail servers ----

struct World {
    imap: Running<MailHandle>,
    smtp: Running<MailHandle>,
}

async fn world() -> World {
    let accounts = Accounts::password(ADDRESS, PASSWORD);
    World {
        imap: FakeImap::start(&Bind::Port(0), Tls::Plain, accounts.clone(), mailbox(1))
            .await
            .expect("imap"),
        smtp: FakeSmtp::start(&Bind::Port(0), Tls::Plain, accounts)
            .await
            .expect("smtp"),
    }
}

fn generic_service(
    sheets: ScriptedSheets,
) -> AccountService<Box<dyn DynProvider>, MemorySecrets, ScriptedSheets, FixedClock> {
    let spec = parse_provider(include_str!("../../../../providers/generic-imap.toml"))
        .expect("the shipped file");
    let provider = GenericProvider::<MailLookup>::new(
        spec,
        SharedHttp::new(Fakes::loopback()),
        FakeDns::new().unreachable(),
    )
    .with_connect(porter_proxy::RustlsConnect::trusting([]));
    AccountService::new(
        vec![DynProvider::boxed(provider)],
        Registry::default(),
        MemorySecrets::default(),
        sheets,
        FixedClock(NOW),
    )
}

/// The edit form of `current` sent as prefilled, with `changes` over it.
fn submit(current: &Manual, changes: Vec<porter_sheet::FieldAnswer>) -> SheetInput {
    let mut answers: Vec<porter_sheet::FieldAnswer> =
        edit_form(porter_core::AuthKind::Password, current, Some(""))
            .iter()
            .filter(|f| changes.iter().all(|c| c.kind != f.kind))
            .filter(|f| f.entry == porter_sheet::Entry::Plain)
            .map(|f| plain(f.kind, f.prefill.as_deref().unwrap_or("")))
            .collect();
    answers.extend(changes);
    SheetInput::Submit(answers)
}

#[tokio::test]
async fn the_typed_servers_are_tried_and_handed_back_with_the_name_and_the_password_once() {
    let world = world().await;
    let current = servers(1, 2);
    let (imap, smtp) = (port_of(world.imap.address()), port_of(world.smtp.address()));
    let typed = vec![
        plain(FieldKind::Server, "127.0.0.1"),
        plain(FieldKind::Port, &imap.to_string()),
        plain(FieldKind::OutgoingPort, &smtp.to_string()),
        plain(FieldKind::Name, "Ada Lovelace"),
        secret(FieldKind::Password, PASSWORD),
    ];
    let sheets = ScriptedSheets::default().conversing([vec![submit(&current, typed)]]);
    let log = sheets.log();
    let service = generic_service(sheets);
    let edit = request(&AccountId::parse("mail-ada").expect("id"), "generic-imap", current)
        .with_name("");

    let edited = service
        .accounts_api()
        .edit_servers(&app(), edit, ParentWindow::Unparented)
        .await
        .expect("edited");

    assert_eq!(edited.servers(), &servers(imap, smtp));
    assert_eq!(edited.name(), Some("Ada Lovelace"));
    assert!(!format!("{edited:?}").contains(PASSWORD), "{edited:?}");
    let shown = format!("{:?}", log.shown());
    assert!(!shown.contains(PASSWORD), "no view carries the password");
    let (_, _, password) = edited.into_parts();
    assert_eq!(password.as_ref().map(SecretText::expose), Some(PASSWORD));
    // Nothing was stored: the service still holds no account.
    assert!(service.registry().accounts.is_empty());
    let attempts = [world.imap.attempts(), world.smtp.attempts()].concat();
    assert!(attempts.len() >= 2 && attempts.iter().all(|a| a.accepted), "{attempts:?}");
    assert!(
        attempts.iter().all(|a| matches!(a.secret, Secret::Password(_))),
        "no bearer token reaches a typed host: {attempts:?}"
    );
}

#[tokio::test]
async fn a_refused_password_marks_the_field_and_the_person_sends_again() {
    let world = world().await;
    let current = servers(port_of(world.imap.address()), port_of(world.smtp.address()));
    let wrong = submit(&current, vec![secret(FieldKind::Password, "wrong-pw-example")]);
    let right = submit(&current, vec![secret(FieldKind::Password, PASSWORD)]);
    let sheets = ScriptedSheets::default().conversing([vec![wrong, right]]);
    let log = sheets.log();
    let service = generic_service(sheets);
    let edit = request(&AccountId::parse("mail-ada").expect("id"), "generic-imap", current);

    let edited = service
        .accounts_api()
        .edit_servers(&app(), edit, ParentWindow::Unparented)
        .await
        .expect("edited on the second send");

    assert!(edited.has_password());
    let marked: Vec<_> = log
        .shown()
        .into_iter()
        .filter_map(|view| match view {
            SheetView::SignIn(form) => form.problem,
            _ => None,
        })
        .collect();
    assert_eq!(marked.len(), 1, "{marked:?}");
    assert_eq!(marked[0].field, FieldKind::Password);
    assert_eq!(marked[0].problem, ProblemKind::Refused);
}

#[tokio::test]
async fn a_cancelled_edit_gives_nothing_back_and_tries_nothing() {
    let world = world().await;
    let current = servers(port_of(world.imap.address()), port_of(world.smtp.address()));
    let sheets = ScriptedSheets::default().conversing([vec![SheetInput::Dismiss]]);
    let service = generic_service(sheets);
    let edit = request(&AccountId::parse("mail-ada").expect("id"), "generic-imap", current);

    let got = service
        .accounts_api()
        .edit_servers(&app(), edit, ParentWindow::Unparented)
        .await;

    assert_eq!(got.err(), Some(EditError::Refused(Refusal::Dismissed)));
    assert!(world.imap.attempts().is_empty());
}
