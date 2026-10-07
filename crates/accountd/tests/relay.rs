//! `Tokens.OpenAuthenticated` on a private bus with scratch HOME and XDG (design/31 §7.1 item 9):
//! an app holding a grant for an app-password IMAP account gets a socket to a relay that logs in
//! for it, against `FakeImap` over TLS from the fake CA (never the system store), and the
//! password is in no bus message. Refusals bring no descriptor.

mod common;

use accountd::{Callers, Host, Options, RelayRoots, TableCallers, serve_with};
use common::bus::PrivateBus;
use common::{APP_PASSWORD, Shared, app, caller, error_name, refusal_name};
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::wire::Refusal;
use porter_core::{
    Account, AuthKind, CapabilityKind, Credential, DataClass, GrantId, SecretKey, SecretPurpose,
    SecretText, SpaceScope, UnixSeconds,
};
use porter_dbus::{CallerRole, TokensProxy};
use porter_fake::{FixedClock, MemoryStore, RecordingAudit, mail_account, mail_provider};
use porter_fake_servers::net::Bind;
use porter_fake_servers::{FakeImap, MailEvent, Running, Secret, mailbox, tls};
use porter_secrets::Secrets;
use porter_service::{AccountService, Registry};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zbus::export::futures_core::Stream;
use zbus::fdo::MonitoringProxy;

const USER: &str = "alice@fake.test";
const GRANT: &str = "relay-grant";

type Fake = Running<porter_fake_servers::MailHandle>;

/// The fake IMAP server (implicit TLS, one mailbox of three) and its port.
async fn imap() -> (Fake, u16) {
    let accounts = porter_fake_servers::Accounts::password(USER, APP_PASSWORD);
    let fake = FakeImap::start(
        &Bind::Loopback,
        porter_core::Tls::Implicit,
        accounts,
        mailbox(3),
    )
    .await
    .expect("imap");
    let port = match fake.address() {
        porter_fake::FakeAddress::Loopback(port) => *port,
        porter_fake::FakeAddress::Socket(_) => panic!("a loopback fake"),
    };
    (fake, port)
}

fn fake_ca() -> RelayRoots {
    RelayRoots::Only(vec![tls::ca_der()])
}

fn imap_url(port: u16) -> String {
    format!("imaps://127.0.0.1:{port}")
}

/// `fake-mail` as an app-password IMAP account at the fake.
fn account(port: u16) -> Account {
    let mut account = mail_account();
    account.auth = AuthKind::AppPassword;
    account
        .endpoints
        .retain(|e| e.family == porter_core::Family::Imap);
    account.endpoints[0].url = porter_core::EndpointUrl::parse(&imap_url(port)).expect("url");
    account.endpoints[0].login = porter_core::LoginName(USER.into());
    account
}

fn grant_for(app: porter_core::AppId, account: &Account, scope: GrantScope) -> Grant {
    Grant {
        id: GrantId::parse(GRANT).expect("id"),
        key: GrantKey {
            app,
            account: account.id.clone(),
            kind: CapabilityKind::Mail,
            class: DataClass::Mail,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
        },
        decision: Decision::Allow,
        scope,
        at: UnixSeconds(1),
    }
}

/// accountd over the real service, the app-password account and a grant for `holder`.
async fn serve<C: Callers>(
    bus: &PrivateBus,
    callers: Arc<C>,
    holder: porter_core::AppId,
    (port, stored_password): (u16, &str),
    roots: RelayRoots,
) -> (zbus::Connection, Arc<impl Host + use<C>>) {
    let stored = Credential::Password(SecretText::new(stored_password));
    serve_account(bus, callers, holder, (account(port), stored), roots).await
}

/// As `serve`, for any account and the credential filed for it.
async fn serve_account<C: Callers>(
    bus: &PrivateBus,
    callers: Arc<C>,
    holder: porter_core::AppId,
    held: (Account, Credential),
    roots: RelayRoots,
) -> (zbus::Connection, Arc<impl Host + use<C>>) {
    serve_scoped(bus, callers, holder, held, GrantScope::Always, roots).await
}

/// As `serve_account`, the grant held `scope`.
async fn serve_scoped<C: Callers>(
    bus: &PrivateBus,
    callers: Arc<C>,
    holder: porter_core::AppId,
    (account, stored): (Account, Credential),
    scope: GrantScope,
    roots: RelayRoots,
) -> (zbus::Connection, Arc<impl Host + use<C>>) {
    let connection = bus.connect().await;
    let secrets = Shared::default();
    secrets
        .put(
            &SecretKey {
                account: account.id.clone(),
                purpose: SecretPurpose::Password,
            },
            &stored,
        )
        .await
        .expect("secret");
    let registry = Registry {
        grants: vec![grant_for(holder, &account, scope)],
        accounts: vec![account],
        toggles: vec![],
    };
    let sheets = accountd::BusSheets::new(connection.clone(), Arc::clone(&callers));
    let service = Arc::new(
        AccountService::new(
            vec![mail_provider()],
            registry,
            secrets,
            sheets,
            FixedClock(porter_fake::NOW),
        )
        .with_store(MemoryStore::default())
        .with_audit(RecordingAudit::default()),
    );
    let options = Options {
        relay_roots: roots,
        ..Options::default()
    };
    serve_with(&connection, Arc::clone(&service), callers, options)
        .await
        .expect("accountd serves");
    (connection, service)
}

/// The app's end of a relay, as a tokio stream.
fn stream_of(fd: zbus::zvariant::OwnedFd) -> tokio::net::UnixStream {
    let std_stream = std::os::unix::net::UnixStream::from(std::os::fd::OwnedFd::from(fd));
    std_stream.set_nonblocking(true).expect("nonblocking");
    tokio::net::UnixStream::from_std(std_stream).expect("tokio stream")
}

/// Reads until `needle` has arrived.
async fn read_until(stream: &mut tokio::net::UnixStream, seen: &mut String, needle: &str) {
    let mut buf = [0u8; 4096];
    while !seen.contains(needle) {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut buf))
            .await
            .expect("the relay answers in time")
            .expect("read");
        assert!(n > 0, "the relay closed before `{needle}`: {seen}");
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
}

fn connected(fake: &Fake) -> usize {
    fake.events()
        .iter()
        .filter(|e| matches!(e, MailEvent::Connected { .. }))
        .count()
}

async fn tokens(connection: &zbus::Connection) -> TokensProxy<'_> {
    TokensProxy::new(connection).await.expect("proxy")
}

/// The one table rig: apps and agents by unique name.
async fn table_rig(
    port: u16,
    password: &str,
) -> (
    PrivateBus,
    (zbus::Connection, Arc<impl Host>),
    Arc<TableCallers>,
) {
    let bus = PrivateBus::start();
    let callers = Arc::new(TableCallers::new());
    let server = serve(
        &bus,
        Arc::clone(&callers),
        app("org.quire.Mail"),
        (port, password),
        fake_ca(),
    )
    .await;
    (bus, server, callers)
}

async fn client(
    bus: &PrivateBus,
    callers: &TableCallers,
    name: &str,
    role: CallerRole,
) -> zbus::Connection {
    let connection = bus.connect().await;
    callers.introduce_as(
        connection.unique_name().expect("name").as_str(),
        caller(name, role),
    );
    connection
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_with_a_grant_reads_its_mail_through_a_relay_that_logs_in_for_it() {
    let (fake, port) = imap().await;
    let (bus, _accountd, callers) = table_rig(port, APP_PASSWORD).await;
    let mail = client(&bus, &callers, "org.quire.Mail", CallerRole::App).await;

    let fd = tokens(&mail)
        .await
        .open_authenticated(GRANT, &imap_url(port))
        .await
        .expect("a relay");
    let mut app = stream_of(fd);
    let mut seen = String::new();
    read_until(&mut app, &mut seen, "\r\n").await;
    assert!(seen.starts_with("* PREAUTH"), "{seen}");
    app.write_all(b"a1 SELECT INBOX\r\n").await.expect("write");
    read_until(&mut app, &mut seen, "a1 OK").await;
    assert!(seen.contains("* 3 EXISTS"), "{seen}");
    app.write_all(b"a2 FETCH 2 (BODY[])\r\n")
        .await
        .expect("write");
    read_until(&mut app, &mut seen, "a2 OK").await;
    assert!(seen.contains("Subject: message 2"), "{seen}");
    assert!(!seen.contains(APP_PASSWORD));

    let attempts = fake.attempts();
    assert_eq!(attempts.len(), 1, "one login, made by the relay");
    assert_eq!(attempts[0].user, USER);
    assert_eq!(attempts[0].secret, Secret::Password(APP_PASSWORD.into()));
    assert!(attempts[0].accepted);
    assert_eq!(
        connected(&fake),
        1,
        "the app never dialed the server itself"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_is_denied_and_nothing_is_dialed() {
    let (fake, port) = imap().await;
    let (bus, _accountd, callers) = table_rig(port, APP_PASSWORD).await;
    // The agent holds the grant by name: the role is checked first.
    let agent = client(&bus, &callers, "org.quire.Mail", CallerRole::Agent).await;
    let denied = tokens(&agent)
        .await
        .open_authenticated(GRANT, &imap_url(port))
        .await
        .expect_err("denied");
    assert_eq!(error_name(&denied), refusal_name(Refusal::Denied));
    assert_eq!(connected(&fake), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_sender_is_access_denied() {
    let (_fake, port) = imap().await;
    let (bus, _accountd, _callers) = table_rig(port, APP_PASSWORD).await;
    let stranger = bus.connect().await;
    let refused = tokens(&stranger)
        .await
        .open_authenticated(GRANT, &imap_url(port))
        .await
        .expect_err("refused");
    assert_eq!(error_name(&refused), common::ACCESS_DENIED);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_endpoint_the_grant_does_not_cover_is_refused_and_not_dialed() {
    let (fake, port) = imap().await;
    let (bus, _accountd, callers) = table_rig(port, APP_PASSWORD).await;
    let mail = client(&bus, &callers, "org.quire.Mail", CallerRole::App).await;
    let other = port.wrapping_add(1);
    let cases = [
        (GRANT, imap_url(other), Refusal::EndpointNotGranted),
        (
            GRANT,
            "imaps://elsewhere.invalid".to_owned(),
            Refusal::EndpointNotGranted,
        ),
        ("no-such-grant", imap_url(port), Refusal::UnknownGrant),
    ];
    for (grant, endpoint, refusal) in cases {
        let error = tokens(&mail)
            .await
            .open_authenticated(grant, &endpoint)
            .await
            .expect_err("refused");
        assert_eq!(error_name(&error), refusal_name(refusal), "{endpoint}");
    }
    // Another app does not hold the grant.
    let photos = client(&bus, &callers, "org.quire.Photos", CallerRole::App).await;
    let error = tokens(&photos)
        .await
        .open_authenticated(GRANT, &imap_url(port))
        .await
        .expect_err("refused");
    assert_eq!(error_name(&error), refusal_name(Refusal::UnknownGrant));
    assert_eq!(connected(&fake), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_password_the_server_refuses_is_a_refusal_and_no_descriptor() {
    let (fake, port) = imap().await;
    let (bus, accountd, callers) = table_rig(port, "not-the-password").await;
    let mail = client(&bus, &callers, "org.quire.Mail", CallerRole::App).await;
    let error = tokens(&mail)
        .await
        .open_authenticated(GRANT, &imap_url(port))
        .await
        .expect_err("no descriptor");
    assert_eq!(error_name(&error), refusal_name(Refusal::NeedsReauth));
    let states: Vec<_> = accountd
        .1
        .registry()
        .accounts
        .iter()
        .map(|a| a.state)
        .collect();
    assert_eq!(states, [porter_core::AccountState::NeedsReauth]);
    let attempts = fake.attempts();
    assert_eq!(attempts.len(), 1);
    assert!(!attempts[0].accepted);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_no_trusted_root_vouches_for_is_unavailable_and_gets_no_password() {
    let (fake, port) = imap().await;
    let bus = PrivateBus::start();
    let callers = Arc::new(TableCallers::new());
    // The fake's CA is not among the roots: the platform store is never consulted either.
    let _accountd = serve(
        &bus,
        Arc::clone(&callers),
        app("org.quire.Mail"),
        (port, APP_PASSWORD),
        RelayRoots::Only(Vec::new()),
    )
    .await;
    let mail = client(&bus, &callers, "org.quire.Mail", CallerRole::App).await;
    let error = tokens(&mail)
        .await
        .open_authenticated(GRANT, &imap_url(port))
        .await
        .expect_err("no descriptor");
    assert_eq!(error_name(&error), refusal_name(Refusal::Unavailable));
    assert!(fake.attempts().is_empty());
}

/// A /proc tree naming this process a Flatpak app.
#[cfg(feature = "test-proc-root")]
fn proc_tree(dir: &std::path::Path) -> std::path::PathBuf {
    let root = dir.join("proc");
    let pid = root.join(std::process::id().to_string());
    std::fs::create_dir_all(&pid).expect("proc dir");
    std::fs::write(
        pid.join("cgroup"),
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-org.example.Probe-1.scope\n",
    )
    .expect("cgroup");
    root
}

/// Every message on the bus, from the moment the monitor is set.
struct Tap(zbus::MessageStream);

impl Tap {
    async fn start(bus: &PrivateBus) -> Self {
        let monitor = bus.connect().await;
        MonitoringProxy::new(&monitor)
            .await
            .expect("proxy")
            .become_monitor(&[], 0)
            .await
            .expect("monitor");
        Self(zbus::MessageStream::from(monitor))
    }

    async fn drain(&mut self) -> Vec<Vec<u8>> {
        let mut seen = Vec::new();
        loop {
            let next = tokio::time::timeout(
                std::time::Duration::from_millis(300),
                std::future::poll_fn(|cx| std::pin::Pin::new(&mut self.0).poll_next(cx)),
            )
            .await;
            match next {
                Ok(Some(Ok(message))) => seen.push(message.data().to_vec()),
                _ => return seen,
            }
        }
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// Acceptance 9 (design/31 §7.1, PLAN §8): a Flatpak app, identified by its cgroup, opens a relay
/// to the fake IMAP server, SELECTs and FETCHes, and the password is in no bus message.
#[cfg(feature = "test-proc-root")]
#[tokio::test(flavor = "multi_thread")]
async fn acceptance_9_a_flatpak_app_reads_mail_and_no_bus_message_holds_the_password() {
    let (fake, port) = imap().await;
    let bus = PrivateBus::start();
    let root = proc_tree(bus.scratch());
    let connection = bus.connect().await;
    let callers = Arc::new(porter_dbus::ProcCallers::with_proc_root(
        connection.clone(),
        porter_dbus::CallerTable::default(),
        root,
    ));
    let _ = connection;
    let _accountd = serve(
        &bus,
        callers,
        app("org.example.Probe"),
        (port, APP_PASSWORD),
        fake_ca(),
    )
    .await;
    let mut tap = Tap::start(&bus).await;

    let probe = bus.connect().await;
    let fd = tokens(&probe)
        .await
        .open_authenticated(GRANT, &imap_url(port))
        .await
        .expect("a relay");
    let mut app = stream_of(fd);
    let mut seen = String::new();
    read_until(&mut app, &mut seen, "\r\n").await;
    assert!(seen.starts_with("* PREAUTH"), "{seen}");
    app.write_all(b"a1 SELECT INBOX\r\n").await.expect("write");
    read_until(&mut app, &mut seen, "a1 OK").await;
    app.write_all(b"a2 FETCH 1 (BODY[])\r\n")
        .await
        .expect("write");
    read_until(&mut app, &mut seen, "a2 OK").await;
    assert!(seen.contains("Subject: message 1"), "{seen}");
    assert_eq!(fake.attempts().len(), 1);

    // Positive control: the tap does see a string put on the bus.
    let _ = tokens(&probe).await.issue_token(GRANT, APP_PASSWORD).await;
    let messages = tap.drain().await;
    assert!(
        messages.len() > 2,
        "the tap saw {} messages",
        messages.len()
    );
    assert!(
        messages.iter().any(|m| contains(m, APP_PASSWORD)),
        "the positive control must be found"
    );
    // Without the control message, nothing holds it.
    let clean: Vec<_> = messages
        .iter()
        .filter(|m| !contains(m, "IssueToken"))
        .collect();
    assert!(clean.len() >= 2, "the open call and its reply were seen");
    for message in clean {
        assert!(!contains(message, APP_PASSWORD));
    }
}

/// `fake-mail` as a password POP3 account at the fake, over implicit TLS.
fn pop3_account(port: u16) -> Account {
    let mut account = account(port);
    account.auth = AuthKind::Password;
    account.endpoints[0].family = porter_core::Family::Pop3;
    account.endpoints[0].url =
        porter_core::EndpointUrl::parse(&format!("pop3s://127.0.0.1:{port}")).expect("url");
    account
}

async fn pop3() -> (Fake, u16) {
    let accounts = porter_fake_servers::Accounts::password(USER, APP_PASSWORD).with_bearer("api-9");
    let fake = porter_fake_servers::FakePop3::start(
        &Bind::Loopback,
        porter_core::Tls::Implicit,
        accounts,
        mailbox(3),
    )
    .await
    .expect("pop3");
    let port = match fake.address() {
        porter_fake::FakeAddress::Loopback(port) => *port,
        porter_fake::FakeAddress::Socket(_) => panic!("a loopback fake"),
    };
    (fake, port)
}

/// An app opens the POP3 relay of `account` filed with `stored`; what the fake saw is returned.
async fn through_pop3(account: Account, stored: Credential, fake: &Fake, port: u16) -> String {
    let bus = PrivateBus::start();
    let callers = Arc::new(TableCallers::new());
    let _server = serve_account(
        &bus,
        Arc::clone(&callers),
        app("org.quire.Mail"),
        (account, stored),
        fake_ca(),
    )
    .await;
    let mail = client(&bus, &callers, "org.quire.Mail", CallerRole::App).await;
    let fd = tokens(&mail)
        .await
        .open_authenticated(GRANT, &format!("pop3s://127.0.0.1:{port}"))
        .await
        .expect("a relay");
    let mut app = stream_of(fd);
    let mut seen = String::new();
    read_until(&mut app, &mut seen, "\r\n").await;
    assert!(seen.starts_with("+OK porter relay ready"), "{seen}");
    app.write_all(b"RETR 2\r\n").await.expect("write");
    read_until(&mut app, &mut seen, "\r\n.\r\n").await;
    assert!(seen.contains("Subject: message 2"), "{seen}");
    assert!(!seen.contains(APP_PASSWORD) && !seen.contains("api-9"));
    assert_eq!(connected(fake), 1, "the app never dialed the server itself");
    seen
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pop3_account_is_read_through_a_relay_that_logs_in_with_its_password() {
    let (fake, port) = pop3().await;
    let stored = Credential::Password(SecretText::new(APP_PASSWORD));
    through_pop3(pop3_account(port), stored, &fake, port).await;
    let attempts = fake.attempts();
    assert_eq!(attempts.len(), 1, "one login, made by the relay");
    assert_eq!(attempts[0].user, USER);
    assert_eq!(attempts[0].secret, Secret::Password(APP_PASSWORD.into()));
    assert!(attempts[0].accepted);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stored_api_token_is_presented_as_a_bearer() {
    let (fake, port) = pop3().await;
    let stored = Credential::Bearer(SecretText::new("api-9"));
    through_pop3(pop3_account(port), stored, &fake, port).await;
    let attempts = fake.attempts();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].mechanism,
        porter_fake_servers::Mechanism::Xoauth2
    );
    assert_eq!(attempts[0].secret, Secret::Bearer("api-9".into()));
    assert!(attempts[0].accepted);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_allow_once_grant_is_spent_by_the_relay_it_opens_and_the_second_open_is_refused() {
    let (fake, port) = imap().await;
    let bus = PrivateBus::start();
    let callers = Arc::new(TableCallers::new());
    let stored = Credential::Password(SecretText::new(APP_PASSWORD));
    let (_connection, accountd) = serve_scoped(
        &bus,
        Arc::clone(&callers),
        app("org.quire.Mail"),
        (account(port), stored),
        GrantScope::Once,
        fake_ca(),
    )
    .await;
    let mail = client(&bus, &callers, "org.quire.Mail", CallerRole::App).await;
    assert_eq!(accountd.registry().grants.len(), 1);

    let fd = tokens(&mail)
        .await
        .open_authenticated(GRANT, &imap_url(port))
        .await
        .expect("the first open is the grant's one use");
    drop(stream_of(fd));
    assert!(
        accountd.registry().grants.is_empty(),
        "the relay spent the grant"
    );

    let again = tokens(&mail)
        .await
        .open_authenticated(GRANT, &imap_url(port))
        .await
        .expect_err("a spent grant opens nothing");
    assert_eq!(error_name(&again), refusal_name(Refusal::UnknownGrant));
    let token = tokens(&mail)
        .await
        .issue_token(GRANT, "mail")
        .await
        .expect_err("nor does it issue a token");
    assert_eq!(error_name(&token), refusal_name(Refusal::UnknownGrant));
    assert_eq!(fake.attempts().len(), 1, "only the first open dialed");
}
