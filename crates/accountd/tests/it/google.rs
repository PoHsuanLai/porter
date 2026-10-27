//! A Google account end to end on a private bus: accountd over the real service and the real
//! Google family, a fake Google (its issuer and account APIs) on loopback, a sheet host written
//! by hand that does what a person would (picks nothing, follows the browser step, confirms the
//! review), and apps as clients. Adding it, its capabilities, signing in again, the seven days a
//! client in testing lasts, removing it (which revokes at Google), and no secret of the account's
//! on the bus. (Tokens and relays for apps are no longer accountd's: an app signs in through
//! `Identities1` and keeps its own tokens.)

use crate::common;

use accountd::{BusSheets, Options, TableCallers, serve_with};
use common::bus::PrivateBus;
use common::host::{HostLog, SheetHost, send_input};
use common::{Shared, Tap, caller, contains};
use porter_core::capability::CapabilityKind;
use porter_core::clock::FixedClock;
use porter_core::sheet::{SheetInput, SheetView};
use porter_core::{
    AccountId, AccountState, Credential, Offer, ProviderId, SecretKey, SecretPurpose, TokenLifetime,
};
use porter_dbus::callers::CallerRole;
use porter_dbus::{ManagerProxy, Sheet};
use porter_fake::{MemoryStore, RecordingAudit};
use porter_fake_servers::{Google, shipped};
use porter_families::{FamilyProvider, GoogleEnv, GoogleProvider};
use porter_http::HyperHttp;
use porter_oauth::{AppReview, ClientRegistry, ClientTraits, MailRights};
use porter_provider::{ClientChannel, ClientEntry, ClientId, ClientsFile, Issuer};
use porter_secrets::Secrets;
use porter_service::{AccountService, Registry};
use std::sync::Arc;
use std::time::Duration;

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

impl porter_core::clock::Clock for Sky {
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

impl World {
    async fn start(traits: ClientTraits) -> Self {
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
            .at(porter_dbus::names::SHEET_PATH, host)
            .await
            .expect("host object");
        porter_dbus::serve_ready(&host_connection)
            .await
            .expect("the host takes calls");
        host_connection
            .request_name(porter_dbus::names::SHEET_BUS)
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
            accounts: vec![],
            grants: vec![],
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
                clock: Some(sky.clone()),
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
        self.connection_as(name, CallerRole::App).await
    }

    async fn connection_as(&self, name: &str, role: CallerRole) -> zbus::Connection {
        let connection = self.bus.connect().await;
        let unique = connection.unique_name().expect("name").to_string();
        self.callers.introduce_as(&unique, caller(name, role));
        connection
    }

    /// Plays the person at the sheet host until `done` resolves: picks Google, follows the
    /// browser step, confirms the review.
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

    /// Settings' client, as the Settings app.
    async fn settings(&self) -> ds_settings::live::LiveClient {
        let connection = self
            .connection_as("org.quire.Settings", CallerRole::Settings)
            .await;
        ds_settings::live::LiveClient::new(
            &connection,
            "org.quire.Accounts1",
            &accountd::settings_path(),
            ds_settings::live::AnyOwner,
        )
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

/// `ReauthReason`, read as the shell does (no app reads an account's properties).
async fn reauth_reason_of(world: &World, id: &AccountId) -> String {
    let shell = world
        .connection_as("org.example.OtherShell", CallerRole::SheetHost)
        .await;
    porter_dbus::AccountProxy::builder(&shell)
        .path(
            zbus::zvariant::ObjectPath::try_from(porter_dbus::names::account_path(id))
                .expect("path"),
        )
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
    let world = World::start(TESTING).await;
    let mail = world.app("org.quire.Mail").await;
    let id = world.add(&mail).await;
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

    // Settings: a read-only row with the date.
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
    assert_eq!(reauth_reason_of(&world, &id).await, "");

    // A sign-out a day after signing in is not blamed on the seven days.
    world.sky.0.store(signed_in + DAY, SeqCst);
    assert!(
        world
            .service
            .set_state(&id, AccountState::NeedsReauth)
            .await
    );
    assert_eq!(reauth_reason_of(&world, &id).await, "");
    sign_in_again(&world, &id).await;
    assert_eq!(reauth_reason_of(&world, &id).await, "");

    // A sign-out after the seven days is Google's: the word, and the row says it happened.
    let again = world.account().restriction.signed_in.expect("renewed").0;
    world.sky.0.store(again + 8 * DAY, SeqCst);
    assert!(
        world
            .service
            .set_state(&id, AccountState::NeedsReauth)
            .await
    );
    assert_eq!(reauth_reason_of(&world, &id).await, "testing_app_expired");
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
    assert_eq!(reauth_reason_of(&world, &id).await, "");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_google_account_is_added_with_its_capabilities_and_no_secret_on_the_bus() {
    let world = World::start(TESTING).await;
    let mut tap = Tap::start(&world.bus).await;
    let mail = world.app("org.quire.Mail").await;

    let id = world.add(&mail).await;
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
    let status = world
        .google
        .api
        .hits()
        .iter()
        .filter(|h| h.status == 200)
        .count();
    assert!(status >= 5, "the sign-in probed the services: {status}");

    // The application secret went to Google, and neither it nor the refresh token went anywhere
    // else: no message on the bus carries them.
    for message in tap.drain().await {
        assert!(!contains(&message, &refresh));
        assert!(!contains(&message, SECRET));
    }
    let audit = format!("{:?}", world.audit.entries());
    assert!(!audit.contains(&refresh) && !audit.contains(SECRET));
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_in_again_from_settings_renews_the_refresh_token() {
    let world = World::start(TESTING).await;
    let mail = world.app("org.quire.Mail").await;
    let id = world.add(&mail).await;
    let first = world.refresh_of(&id).await;
    assert!(
        world
            .service
            .set_state(&id, AccountState::NeedsReauth)
            .await
    );
    // Signing in again from Settings: the browser, no review, and the account works again.
    sign_in_again(&world, &id).await;
    let account = world.account();
    assert_eq!(account.state, AccountState::Ok);
    let refreshed = world.refresh_of(&id).await;
    assert!(world.google.issuer.refresh_is_live(&refreshed));
    assert_ne!(first, refreshed, "a new sign-in is a new refresh token");
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_the_account_revokes_it_at_google_and_forgets_its_secrets() {
    let world = World::start(TESTING).await;
    let mail = world.app("org.quire.Mail").await;
    let id = world.add(&mail).await;
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
