//! A Google account end to end on a private bus: accountd over the real service and the real
//! Google family, a fake Google (its issuer and account APIs) on loopback, a sheet host written
//! by hand that does what a person would (picks nothing, follows the browser step, confirms the
//! review), and apps as clients. Adding it, its capabilities and grants, a token for a calendar
//! grant, a refresh, Google refusing the refresh, signing in again, removing it (which revokes at
//! Google), Gmail through the relay with XOAUTH2 for a client of one's own, and no secret of the
//! account's on the bus.

use crate::common;

use accountd::{BusSheets, Options, RelayRoots, TableCallers, serve_with};
use common::bus::PrivateBus;
use common::host::{HostLog, SheetHost, send_input};
use common::{Shared, Tap, caller, contains, error_name, refusal_name};
use porter_core::capability::{Access, CapabilityKind, Delta};
use porter_core::consent::{ConsentAnswer, GrantScope};
use porter_core::need::PimNeed;
use porter_core::sheet::{SheetInput, SheetView};
use porter_core::wire::Refusal;
use porter_core::{
    AccountId, AccountState, Credential, Need, Offer, ProviderId, SecretKey, SecretPurpose,
    TokenLifetime,
};
use porter_dbus::{CallerRole, ManagerProxy, Sheet, TokensProxy, need_to_dbus};
use porter_fake::{FixedClock, MemoryStore, RecordingAudit};
use porter_fake_servers::net::Bind;
use porter_fake_servers::{FakeImap, Google, Secret, mailbox, shipped, tls};
use porter_families::{FamilyProvider, GoogleEnv, GoogleProvider};
use porter_http::HyperHttp;
use porter_oauth::{AppReview, ClientRegistry, ClientTraits, MailRights};
use porter_provider::{ClientChannel, ClientEntry, ClientId, ClientsFile, Issuer};
use porter_secrets::Secrets;
use porter_service::{AccountService, Registry};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SECRET: &str = "GOCSPX-bus-test-application-secret";
const CLIENT_ID: &str = "bus-test.apps.googleusercontent.com";

type Svc = AccountService<
    FamilyProvider,
    Shared,
    BusSheets<TableCallers>,
    FixedClock,
    MemoryStore,
    RecordingAudit,
>;

struct World {
    bus: PrivateBus,
    callers: Arc<TableCallers>,
    service: Arc<Svc>,
    secrets: Shared,
    audit: RecordingAudit,
    google: Google,
    host_connection: zbus::Connection,
    host_log: HostLog,
    _connection: zbus::Connection,
    /// How many opened and updated views the person has already answered.
    cursor: std::sync::Mutex<(usize, usize)>,
    /// accountd's own clock: the system's, until a test moves it.
    sky: Arc<Sky>,
}

/// accountd's clock, movable: 0 is the system clock.
#[derive(Default)]
struct Sky(std::sync::atomic::AtomicI64);

impl porter_service::Clock for Sky {
    fn now(&self) -> porter_core::UnixSeconds {
        match self.0.load(std::sync::atomic::Ordering::SeqCst) {
            0 => {
                let since = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                porter_core::UnixSeconds(i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
            }
            at => porter_core::UnixSeconds(at),
        }
    }
}

fn client_with(google: &Google, traits: ClientTraits) -> ClientRegistry {
    ClientRegistry::layered(
        ClientsFile {
            clients: vec![ClientEntry {
                issuer: Issuer::Google,
                channel: ClientChannel::Development,
                client_id: ClientId(CLIENT_ID.into()),
                client_secret: Some(porter_core::SecretText::new(SECRET)),
                endpoints: Some(google.issuer.endpoints()),
            }],
        },
        ClientsFile::default(),
    )
    .with_traits(Issuer::Google, ClientChannel::Development, traits)
}

const TESTING: ClientTraits = ClientTraits {
    mail: MailRights::Withheld,
    review: AppReview::Testing,
};
const BYO: ClientTraits = ClientTraits {
    mail: MailRights::Byo,
    review: AppReview::Testing,
};

impl World {
    async fn start(
        traits: ClientTraits,
        (accounts, grants): (Vec<porter_core::Account>, Vec<porter_core::consent::Grant>),
        roots: RelayRoots,
    ) -> Self {
        let google = Google::start(SECRET).await.expect("fake google");
        let spec = google.api.rewrite(&shipped::google());
        let env = GoogleEnv::new(
            HyperHttp::default(),
            client_with(&google, traits),
            porter_core::clock::SystemClock,
        )
        .with_channel(ClientChannel::Development)
        .with_poll_slice(Duration::from_millis(50))
        .with_userinfo(porter_core::EndpointUrl::parse(&google.api.userinfo_url()).expect("url"));
        let providers = vec![FamilyProvider::Google(GoogleProvider::with_env(spec, env))];

        let bus = PrivateBus::start();
        let callers = Arc::new(TableCallers::new());
        let connection = bus.connect().await;
        let host = SheetHost::quiet();
        let host_log = host.log();
        let host_connection = bus.connect().await;
        host_connection
            .object_server()
            .at(porter_dbus::SHEET_PATH, host)
            .await
            .expect("host object");
        porter_dbus::serve_ready(&host_connection)
            .await
            .expect("the host takes calls");
        host_connection
            .request_name(porter_dbus::SHEET_BUS)
            .await
            .expect("host name");
        callers.introduce_as(
            host_connection.unique_name().expect("name").as_str(),
            caller("org.example.SheetHost", CallerRole::SheetHost),
        );
        let secrets = Shared::default();
        let audit = RecordingAudit::default();
        let sheets = BusSheets::new(connection.clone(), Arc::clone(&callers));
        let registry = Registry {
            accounts,
            grants,
            toggles: vec![],
        };
        let service = Arc::new(
            AccountService::new(
                providers,
                registry,
                secrets.clone(),
                sheets,
                FixedClock(porter_fake::NOW),
            )
            .with_store(MemoryStore::default())
            .with_audit(audit.clone()),
        );
        let sky = Arc::new(Sky::default());
        serve_with(
            &connection,
            Arc::clone(&service),
            Arc::clone(&callers),
            Options {
                relay_roots: roots,
                login: accountd::LoginTiming {
                    clock: Some(sky.clone()),
                    ..Default::default()
                },
                ..Options::default()
            },
        )
        .await
        .expect("accountd serves");
        Self {
            bus,
            callers,
            service,
            secrets,
            audit,
            google,
            host_connection,
            host_log,
            _connection: connection,
            cursor: std::sync::Mutex::new((0, 0)),
            sky,
        }
    }

    async fn app(&self, name: &str) -> zbus::Connection {
        let connection = self.bus.connect().await;
        let unique = connection.unique_name().expect("name").to_string();
        self.callers
            .introduce_as(&unique, caller(name, CallerRole::App));
        connection
    }

    /// Plays the person at the sheet host until `done` resolves: picks Google, follows the
    /// browser step, confirms the review, allows the first account at a consent alert.
    async fn person<T>(&self, done: impl std::future::Future<Output = T>) -> T {
        tokio::pin!(done);
        let (mut opened, mut updated) = *self.cursor.lock().expect("cursor");
        let mut browsers = Vec::new();
        loop {
            tokio::select! {
                result = &mut done => return result,
                () = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
            let views: Vec<(String, String)> = {
                let calls = self.host_log.calls();
                let new_opened = calls.opened[opened..]
                    .iter()
                    .map(|(h, _, v)| (h.clone(), v.clone()));
                let new_updated = calls.updated[updated..]
                    .iter()
                    .map(|(h, v)| (h.clone(), v.clone()));
                let all: Vec<_> = new_opened.chain(new_updated).collect();
                opened = calls.opened.len();
                updated = calls.updated.len();
                *self.cursor.lock().expect("cursor") = (opened, updated);
                all
            };
            for (handle, text) in views {
                let Ok(view) = serde_json::from_str::<SheetView>(&text) else {
                    continue;
                };
                let input = match view {
                    SheetView::Providers(_) => Some(SheetInput::Pick(
                        ProviderId::parse("google").expect("provider"),
                    )),
                    SheetView::BrowserWait { url, .. } => {
                        let page = url.as_str().to_owned();
                        browsers.push(tokio::spawn(async move {
                            porter_fake_servers::follow(&page).await
                        }));
                        None
                    }
                    SheetView::Review(_) => Some(SheetInput::Confirm(Vec::new())),
                    SheetView::Consent(ask) => ask.accounts.first().map(|choice| {
                        SheetInput::Answer(ConsentAnswer::Allow {
                            account: choice.account.clone(),
                            scope: GrantScope::Always,
                        })
                    }),
                    _ => None,
                };
                if let Some(input) = input {
                    send_input(&self.host_connection, &handle, &input).await;
                }
            }
        }
    }

    /// The account added so far (there is one).
    fn account(&self) -> porter_core::Account {
        let registry = self.service.registry();
        assert_eq!(registry.accounts.len(), 1, "{:?}", registry.accounts);
        registry.accounts[0].clone()
    }

    async fn add(&self, app: &zbus::Connection) -> AccountId {
        let mut sheet = Sheet::subscribe(app).await.expect("subscribe");
        let path = ManagerProxy::new(app)
            .await
            .expect("proxy")
            .add_account("google", "", &sheet.options())
            .await
            .expect("add_account");
        let (code, results) = self.person(sheet.response(&path)).await.expect("response");
        assert_eq!(code, 0, "{results:?}");
        self.account().id
    }

    async fn choose(&self, app: &zbus::Connection, kind: &str) -> (u32, porter_dbus::Details) {
        let need = match kind {
            "calendar" => Need::Calendar(PimNeed {
                access: Access::ReadWrite,
                delta: Delta::Poll,
            }),
            _ => Need::Tasks(PimNeed {
                access: Access::ReadWrite,
                delta: Delta::None,
            }),
        };
        let mut sheet = Sheet::subscribe(app).await.expect("subscribe");
        let path = ManagerProxy::new(app)
            .await
            .expect("proxy")
            .choose(
                &need_to_dbus(&need),
                kind,
                "interactive",
                "",
                &sheet.options(),
            )
            .await
            .expect("choose");
        self.person(sheet.response(&path)).await.expect("response")
    }

    /// Settings' client, as the Settings app.
    async fn settings(&self) -> ds_settings::live::LiveClient {
        let connection = self.bus.connect().await;
        let unique = connection.unique_name().expect("name").to_string();
        self.callers
            .introduce_as(&unique, caller("org.quire.Settings", CallerRole::Settings));
        ds_settings::live::LiveClient::new(
            &connection,
            "org.quire.Accounts1",
            &accountd::settings_path(),
        )
        .await
        .expect("client")
    }

    async fn refresh_of(&self, account: &AccountId) -> String {
        let credential = self
            .secrets
            .get(&SecretKey {
                account: account.clone(),
                purpose: SecretPurpose::OAuthRefresh,
            })
            .await
            .expect("the refresh token is filed");
        let Credential::OAuth { refresh, .. } = credential else {
            panic!("an OAuth credential")
        };
        refresh.expose().to_owned()
    }
}

/// Signs the account in again from Settings, as the person would, and waits for `ok`.
async fn sign_in_again(world: &World, id: &AccountId) {
    let settings = world.settings().await;
    settings
        .invoke(&common::key(&format!("accounts.{id}.reauth")))
        .await
        .expect("the action is accepted");
    world
        .person(async {
            let deadline = porter_fake::Deadline::generous();
            while !deadline.passed() {
                if world.account().state == AccountState::Ok {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            panic!("not signed in again: {:?}", world.account().state);
        })
        .await;
}

async fn reauth_reason_of(app: &zbus::Connection, id: &AccountId) -> String {
    porter_dbus::AccountProxy::builder(app)
        .path(zbus::zvariant::ObjectPath::try_from(porter_dbus::account_path(id)).expect("path"))
        .expect("path")
        .build()
        .await
        .expect("proxy")
        .reauth_reason()
        .await
        .expect("reauth_reason")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_in_testing_shows_when_google_signs_it_out_and_says_why_once_it_did() {
    use std::sync::atomic::Ordering::SeqCst;
    const DAY: i64 = 86_400;
    let world = World::start(TESTING, (vec![], vec![]), RelayRoots::default()).await;
    let calendar = world.app("org.quire.Calendar").await;
    world.google.issuer.set_token_lifetime(1);
    let id = world.add(&calendar).await;
    let (_, results) = world.choose(&calendar, "calendar").await;
    let grant = common::grant_in(&results);
    let signed_in = world
        .account()
        .restriction
        .signed_in
        .expect("sign-in date")
        .0;
    assert_eq!(
        world.account().restriction.expires_at().map(|e| e.0),
        Some(signed_in + 7 * DAY)
    );

    // Settings: a read-only row with the date; the app is told nothing but the closed word.
    let settings = world.settings().await;
    let row = common::key(&format!("accounts.{id}.expires"));
    let words = settings.get(&row).await.expect("the row");
    let words = words.as_str().expect("text").to_owned();
    assert!(
        words.starts_with("Google signs this account out on 20"),
        "{words}"
    );
    assert_eq!(words.len(), "Google signs this account out on ".len() + 10);
    assert!(
        settings
            .set(&row, &toml::Value::String("x".into()))
            .await
            .is_err()
    );
    assert_eq!(reauth_reason_of(&calendar, &id).await, "");

    // A refusal a day after signing in is not blamed on the seven days.
    world.sky.0.store(signed_in + DAY, SeqCst);
    world.google.issuer.refuse_refreshes(1);
    let refused = tokens(&calendar)
        .await
        .issue_token(grant.as_str(), "google_calendar")
        .await;
    assert_eq!(
        error_name(&refused.expect_err("refused")),
        refusal_name(Refusal::NeedsReauth)
    );
    assert_eq!(world.account().state, AccountState::NeedsReauth);
    assert_eq!(reauth_reason_of(&calendar, &id).await, "");
    sign_in_again(&world, &id).await;
    assert_eq!(reauth_reason_of(&calendar, &id).await, "");

    // A refusal after the seven days is Google's: the word, and the row says it happened.
    let again = world.account().restriction.signed_in.expect("renewed").0;
    world.sky.0.store(again + 8 * DAY, SeqCst);
    world.google.issuer.refuse_refreshes(1);
    let refused = tokens(&calendar)
        .await
        .issue_token(grant.as_str(), "google_calendar")
        .await;
    assert_eq!(
        error_name(&refused.expect_err("refused")),
        refusal_name(Refusal::NeedsReauth)
    );
    assert_eq!(
        reauth_reason_of(&calendar, &id).await,
        "testing_app_expired"
    );
    let words = settings.get(&row).await.expect("the row");
    assert!(
        words
            .as_str()
            .expect("text")
            .starts_with("Signed out by Google on 20"),
        "{words}"
    );
    // Signing in again clears the reason.
    sign_in_again(&world, &id).await;
    assert_eq!(reauth_reason_of(&calendar, &id).await, "");
}

async fn tokens(app: &zbus::Connection) -> TokensProxy<'_> {
    TokensProxy::new(app).await.expect("proxy")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_google_account_is_added_granted_and_given_a_token_with_no_secret_on_the_bus() {
    let world = World::start(TESTING, (vec![], vec![]), RelayRoots::default()).await;
    let mut tap = Tap::start(&world.bus).await;
    let calendar = world.app("org.quire.Calendar").await;

    let id = world.add(&calendar).await;
    let account = world.account();
    assert_eq!(account.provider.as_str(), "google");
    assert_eq!(account.label.0, "ada@gmail.com");
    assert_eq!(account.state, AccountState::Ok);
    // A client in testing is unverified and signs out in seven days; Drive and Photos are narrow.
    assert_eq!(account.restriction.token_lifetime, TokenLifetime::SevenDays);
    assert_eq!(account.restriction.limits.len(), 2);
    let present: Vec<CapabilityKind> = account
        .capabilities
        .iter()
        .filter(|c| matches!(c.offer, Offer::Present(_)))
        .map(|c| c.offer.kind())
        .collect();
    assert_eq!(
        present,
        [
            CapabilityKind::Calendar,
            CapabilityKind::Contacts,
            CapabilityKind::Tasks,
            CapabilityKind::Storage,
            CapabilityKind::Photos
        ]
    );
    let refresh = world.refresh_of(&id).await;
    assert!(world.google.issuer.refresh_is_live(&refresh));

    // A grant for the calendar, and a token for it that Google's calendar accepts.
    let (code, results) = world.choose(&calendar, "calendar").await;
    assert_eq!(code, 0, "{results:?}");
    let grant = common::grant_in(&results);
    let (kind, token, _expires) = tokens(&calendar)
        .await
        .issue_token(grant.as_str(), "google_calendar")
        .await
        .expect("a token");
    assert_eq!(kind, "bearer");
    assert!(world.google.issuer.access_is_live(&token));
    let scope = world.google.issuer.access_scope(&token).expect("scope");
    assert!(scope.contains("auth/calendar"), "{scope}");
    let status = world
        .google
        .api
        .hits()
        .iter()
        .filter(|h| h.status == 200)
        .count();
    assert!(status >= 5, "the sign-in probed the services: {status}");
    // Not an audience of a calendar grant.
    for other in ["imap", "google_tasks", "graph"] {
        let refused = tokens(&calendar)
            .await
            .issue_token(grant.as_str(), other)
            .await
            .expect_err(other);
        assert_ne!(error_name(&refused), "", "{other}");
    }

    // The application secret went to Google, and neither it nor the refresh token went anywhere
    // else: no message on the bus carries them. (The access token is the app's, by design.)
    for message in tap.drain().await {
        assert!(!contains(&message, &refresh));
        assert!(!contains(&message, SECRET));
    }
    let audit = format!("{:?}", world.audit.entries());
    assert!(!audit.contains(&refresh) && !audit.contains(SECRET));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refreshed_token_a_refused_refresh_and_signing_in_again_end_at_done() {
    let world = World::start(TESTING, (vec![], vec![]), RelayRoots::default()).await;
    let calendar = world.app("org.quire.Calendar").await;
    // A token that is due the moment it is issued, so every IssueToken asks Google again.
    world.google.issuer.set_token_lifetime(1);
    let id = world.add(&calendar).await;
    let (_, results) = world.choose(&calendar, "calendar").await;
    let grant = common::grant_in(&results);

    let first = tokens(&calendar)
        .await
        .issue_token(grant.as_str(), "google_calendar")
        .await
        .expect("a token")
        .1;
    let second = tokens(&calendar)
        .await
        .issue_token(grant.as_str(), "google_calendar")
        .await
        .expect("a token")
        .1;
    assert_ne!(first, second, "each was refreshed");

    // Google ends the sign-in (seven days on): the app is told to ask the person.
    world.google.issuer.refuse_refreshes(1);
    let refused = tokens(&calendar)
        .await
        .issue_token(grant.as_str(), "google_calendar")
        .await
        .expect_err("refused");
    assert_eq!(error_name(&refused), refusal_name(Refusal::NeedsReauth));
    assert_eq!(world.account().state, AccountState::NeedsReauth);

    // Signing in again from Settings: the browser, no review, and the account works again.
    let settings = world.settings().await;
    let path = common::key(&format!("accounts.{id}.reauth"));
    settings
        .invoke(&path)
        .await
        .expect("the action is accepted");
    // The action returns once it is under way; the person follows the browser step meanwhile.
    world
        .person(async {
            let deadline = porter_fake::Deadline::generous();
            while !deadline.passed() {
                if world.account().state == AccountState::Ok {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            panic!(
                "the account was not signed in again: {:?}",
                world.account().state
            );
        })
        .await;
    let account = world.account();
    assert_eq!(account.state, AccountState::Ok);
    let refreshed = world.refresh_of(&id).await;
    assert!(world.google.issuer.refresh_is_live(&refreshed));
    tokens(&calendar)
        .await
        .issue_token(grant.as_str(), "google_calendar")
        .await
        .expect("a token again");
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_the_account_revokes_it_at_google_and_forgets_its_secrets() {
    let world = World::start(TESTING, (vec![], vec![]), RelayRoots::default()).await;
    let calendar = world.app("org.quire.Calendar").await;
    let id = world.add(&calendar).await;
    let refresh = world.refresh_of(&id).await;
    let settings = world.settings().await;
    settings
        .invoke(&common::key(&format!("accounts.{id}.remove")))
        .await
        .expect("removed");
    assert!(world.service.registry().accounts.is_empty());
    assert!(!world.google.issuer.refresh_is_live(&refresh));
    assert!(world.google.issuer.events().iter().any(|e| matches!(
        e,
        porter_fake_servers::IssuerEvent::Revoke { token, known: true } if *token == refresh
    )));
    assert!(
        world
            .secrets
            .get(&SecretKey {
                account: id,
                purpose: SecretPurpose::OAuthRefresh
            })
            .await
            .is_err()
    );
}

/// A Google account of the person's own client, with mail, filed directly; the refresh token
/// is one the fake issuer holds.
fn gmail_account(id: &AccountId, imap_port: u16) -> porter_core::Account {
    use porter_core::capability::{Capability, LabelModel, MailCap, MailTransport, Offered};
    use porter_core::{
        AccountLabel, AuthKind, Claim, EndpointUrl, Family, LoginName, Provenance, Restriction,
        ServiceEndpoint, Subject, Tls,
    };
    porter_core::Account {
        id: id.clone(),
        provider: ProviderId::parse("google").expect("provider"),
        label: AccountLabel("ada@gmail.com".into()),
        state: AccountState::Ok,
        auth: AuthKind::OAuthPkce,
        capabilities: vec![Claim {
            subject: Subject::Account,
            offer: Offer::Present(Capability::Mail(MailCap {
                access: Access::ReadWrite,
                send: Offered::Present,
                delta: Delta::Push,
                transport: MailTransport::Imap,
                labels: LabelModel::Labels,
            })),
            provenance: Provenance::Discovered,
        }],
        restriction: Restriction::none(),
        endpoints: vec![ServiceEndpoint {
            family: Family::Imap,
            url: EndpointUrl::parse(&format!("imaps://127.0.0.1:{imap_port}")).expect("url"),
            tls: Tls::Implicit,
            login: LoginName("ada@gmail.com".into()),
        }],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn gmail_for_a_persons_own_client_is_read_through_the_relay_with_xoauth2() {
    use porter_core::consent::{Grant, GrantKey, Usage};
    use porter_core::{DataClass, GrantId, SpaceScope, UnixSeconds};
    // The first access token the fake issuer mints is `fake-access-1`; the fake IMAP accepts it.
    let accounts = porter_fake_servers::Accounts::password("ada@gmail.com", "unused")
        .with_bearer("fake-access-1");
    let imap = FakeImap::start(
        &Bind::Loopback,
        porter_core::Tls::Implicit,
        accounts,
        mailbox(3),
    )
    .await
    .expect("imap");
    let port = match imap.address() {
        porter_fake::FakeAddress::Loopback(port) => *port,
        porter_fake::FakeAddress::Socket(_) => panic!("a loopback fake"),
    };
    let id = AccountId::parse("11111111-1111-4111-8111-111111111111").expect("id");
    let grant = Grant {
        id: GrantId::parse("gmail-grant").expect("id"),
        key: GrantKey {
            app: common::app("org.quire.Mail"),
            account: id.clone(),
            kind: CapabilityKind::Mail,
            class: DataClass::Mail,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
        },
        decision: porter_core::consent::Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    };
    let world = World::start(
        BYO,
        (vec![gmail_account(&id, port)], vec![grant]),
        RelayRoots::Only(vec![tls::ca_der()]),
    )
    .await;
    world.google.issuer.seed_refresh_as(
        "seeded-refresh",
        CLIENT_ID,
        "https://mail.google.com/ openid https://www.googleapis.com/auth/userinfo.email",
    );
    world
        .secrets
        .put(
            &SecretKey {
                account: id.clone(),
                purpose: SecretPurpose::OAuthRefresh,
            },
            &Credential::OAuth {
                access: porter_core::SecretText::new("stale"),
                refresh: porter_core::SecretText::new("seeded-refresh"),
                expires_at: UnixSeconds(0),
            },
        )
        .await
        .expect("filed");
    let mail = world.app("org.quire.Mail").await;

    let fd = tokens(&mail)
        .await
        .open_authenticated("gmail-grant", &format!("imaps://127.0.0.1:{port}"))
        .await
        .expect("a relay");
    let std_stream = std::os::unix::net::UnixStream::from(std::os::fd::OwnedFd::from(fd));
    std_stream.set_nonblocking(true).expect("nonblocking");
    let mut app = tokio::net::UnixStream::from_std(std_stream).expect("stream");
    let mut seen = String::new();
    let mut buf = [0u8; 4096];
    app.write_all(b"a1 SELECT INBOX\r\n").await.expect("write");
    while !seen.contains("a1 OK") {
        let n = tokio::time::timeout(porter_fake::GENEROUS, app.read(&mut buf))
            .await
            .expect("in time")
            .expect("read");
        assert!(n > 0, "closed early: {seen}");
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    assert!(seen.contains("* 3 EXISTS"), "{seen}");
    let attempts = imap.attempts();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].mechanism,
        porter_fake_servers::Mechanism::Xoauth2
    );
    assert_eq!(attempts[0].user, "ada@gmail.com");
    assert_eq!(attempts[0].secret, Secret::Bearer("fake-access-1".into()));
    assert!(attempts[0].accepted);
}
