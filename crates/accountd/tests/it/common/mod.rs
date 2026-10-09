//! The rig every bus test of accountd uses: a private bus, accountd's front end over the real
//! `AccountService` with the fake providers, a hand-written sheet host, and clients introduced
//! by unique name. Nothing here reaches the real session, Secret Service or network.
#![allow(dead_code)]

pub mod agents;
pub mod bus;
pub mod host;

pub use host::SheetHost;
pub use porter_fake::{mail_account, storage_account};

use accountd::{BusSheets, Options, SecretsDesk, TableCallers, serve_with};
use bus::PrivateBus;
use host::HostLog;
use porter_core::{
    AccountId, AppId, AppName, Credential, Isolation, SecretKey, SecretPurpose, SecretText,
};
use porter_dbus::{Caller, CallerRole, SHEET_BUS, SHEET_PATH};
use porter_fake::{
    FakeProvider, FixedClock, MemoryStore, RecordingAudit, cloud_provider, llm_account,
    llm_provider, mail_provider,
};
use porter_secrets::{MemorySecrets, Secrets, SecretsError};
use porter_service::{AccountService, Registry};
use std::sync::Arc;

pub const APP_PASSWORD: &str = "S3CRET-APP-PASSWORD";
pub const REFRESH_TOKEN: &str = "S3CRET-REFRESH-TOKEN";
pub const ACCESS_TOKEN: &str = "S3CRET-ACCESS-TOKEN";

/// Secrets the test can still read after they moved into the service.
#[derive(Debug, Clone, Default)]
pub struct Shared(pub Arc<MemorySecrets>);

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
    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        self.0.delete_account(account).await
    }
}

pub type Svc = AccountService<
    FakeProvider,
    Shared,
    BusSheets<TableCallers>,
    FixedClock,
    MemoryStore,
    RecordingAudit,
>;

pub fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("app name"),
        isolation: Isolation::Flatpak,
    }
}

pub fn photos() -> AppId {
    app("org.quire.Photos")
}

pub fn caller(name: &str, role: CallerRole) -> Caller {
    Caller {
        app: app(name),
        role,
    }
}

/// accountd, serving, with a sheet host on the bus.
#[derive(Debug)]
pub struct Rig {
    pub bus: PrivateBus,
    pub connection: zbus::Connection,
    pub callers: Arc<TableCallers>,
    pub service: Arc<Svc>,
    pub secrets: Shared,
    pub audit: RecordingAudit,
    pub store: MemoryStore,
    pub host_connection: zbus::Connection,
    pub host_log: HostLog,
}

impl Rig {
    pub async fn start() -> Self {
        Self::start_with(Options::default(), SheetHost::quiet()).await
    }

    pub async fn start_with(options: Options, host: SheetHost) -> Self {
        let providers = vec![cloud_provider(), mail_provider(), llm_provider()];
        Self::start_over(options, host, providers).await
    }

    /// As `start_with`, over these providers (the accounts are the fake three).
    pub async fn start_over(
        options: Options,
        host: SheetHost,
        providers: Vec<FakeProvider>,
    ) -> Self {
        let accounts = vec![storage_account(), mail_account(), llm_account()];
        Self::start_holding(options, host, providers, accounts).await
    }

    /// As `start_over`, with `accounts` in the registry in place of the fake three.
    pub async fn start_holding(
        options: Options,
        host: SheetHost,
        providers: Vec<FakeProvider>,
        accounts: Vec<porter_core::Account>,
    ) -> Self {
        Self::start_granted(options, host, providers, accounts, Vec::new()).await
    }

    /// As `start_holding`, with `grants` already in the consent store (the person said so
    /// earlier), so a test need not walk a sheet for each scope and app it wants.
    pub async fn start_granted(
        mut options: Options,
        host: SheetHost,
        providers: Vec<FakeProvider>,
        accounts: Vec<porter_core::Account>,
        grants: Vec<porter_core::consent::Grant>,
    ) -> Self {
        let bus = PrivateBus::start();
        let callers = Arc::new(TableCallers::new());
        let connection = bus.connect().await;

        let host_log = host.log();
        let host_connection = bus.connect().await;
        host_connection
            .object_server()
            .at(SHEET_PATH, host)
            .await
            .expect("host object");
        // As the real host must: the name only once the connection answers calls (accountd's
        // `Open` to a host still starting was dropped without an answer).
        porter_dbus::serve_ready(&host_connection)
            .await
            .expect("the host takes calls");
        host_connection
            .request_name(SHEET_BUS)
            .await
            .expect("host name");
        callers.introduce_as(
            host_connection.unique_name().expect("name").as_str(),
            caller("org.example.SheetHost", CallerRole::SheetHost),
        );

        let secrets = Shared::default();
        for (account, purpose, credential) in [
            (
                storage_account().id,
                SecretPurpose::Password,
                Credential::Password(SecretText::new(APP_PASSWORD)),
            ),
            (
                mail_account().id,
                SecretPurpose::OAuthRefresh,
                Credential::OAuth {
                    access: SecretText::new(ACCESS_TOKEN),
                    refresh: SecretText::new(REFRESH_TOKEN),
                    expires_at: porter_core::UnixSeconds(1_790_000_000),
                },
            ),
        ] {
            let _ = secrets
                .put(&SecretKey { account, purpose }, &credential)
                .await;
        }
        let registry = Registry {
            accounts,
            grants,
            toggles: vec![],
        };
        let store = MemoryStore::default();
        let audit = RecordingAudit::default();
        if options.keys.is_none() {
            options.keys = Some(Arc::new(SecretsDesk::new(
                secrets.clone(),
                audit.clone(),
                FixedClock(porter_fake::NOW),
            )));
        }
        options
            .login
            .audit
            .get_or_insert_with(|| Arc::new(audit.clone()));
        let sheets = BusSheets::new(connection.clone(), Arc::clone(&callers));
        let service = Arc::new(
            AccountService::new(
                providers,
                registry,
                secrets.clone(),
                sheets,
                FixedClock(porter_fake::NOW),
            )
            .with_store(store.clone())
            .with_audit(audit.clone()),
        );
        serve_with(
            &connection,
            Arc::clone(&service),
            Arc::clone(&callers),
            options,
        )
        .await
        .expect("accountd serves");
        Self {
            bus,
            connection,
            callers,
            service,
            secrets,
            audit,
            store,
            host_connection,
            host_log,
        }
    }

    /// A new connection that accountd knows as `who`.
    pub async fn client_as(&self, who: Caller) -> zbus::Connection {
        let connection = self.bus.connect().await;
        let name = connection.unique_name().expect("name").to_string();
        self.callers.introduce_as(&name, who);
        connection
    }

    /// A connection that is app `name` in the `App` role.
    pub async fn client(&self, name: &str) -> zbus::Connection {
        self.client_as(caller(name, CallerRole::App)).await
    }

    /// A connection accountd does not know.
    pub async fn stranger(&self) -> zbus::Connection {
        self.bus.connect().await
    }
}

/// Polls `condition` until it holds, up to [`porter_fake::GENEROUS`] by the clock.
pub async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if condition() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    deadline.fail(what);
}

use porter_core::Need;
use porter_core::capability::{Access, Delta, QuotaReport, StorageScope};
use porter_core::consent::{ConsentAnswer, GrantScope};
use porter_core::need::StorageNeed;
use porter_core::sheet::SheetInput;
use porter_core::wire::Refusal;
use porter_dbus::{Details, ManagerProxy, NeedArg, Sheet, need_to_dbus, refusal_error_name};

pub fn storage_need() -> NeedArg {
    need_to_dbus(&Need::Storage(StorageNeed {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        scope: StorageScope::AppFolder,
        quota: QuotaReport::Unreported,
    }))
}

/// A host that allows `fake-storage` always, whatever it is asked.
pub fn allowing_host() -> SheetHost {
    SheetHost::answering(SheetInput::Answer(ConsentAnswer::Allow {
        account: storage_account().id,
        scope: GrantScope::Always,
    }))
}

/// `Manager.Choose` as `connection`, and the Request's response.
pub async fn choose(connection: &zbus::Connection) -> (u32, Details) {
    let mut sheet = Sheet::subscribe(connection).await.expect("subscribe");
    let manager = ManagerProxy::new(connection).await.expect("proxy");
    let path = manager
        .choose(
            &storage_need(),
            "photos",
            "interactive",
            "",
            &sheet.options(),
        )
        .await
        .expect("choose");
    sheet.response(&path).await.expect("response")
}

/// The text of a vardict entry.
pub fn text_of(details: &Details, key: &str) -> Option<String> {
    let value = details.get(key)?.try_clone().ok()?;
    String::try_from(value).ok()
}

/// The grant a `Choose` response carries.
pub fn grant_in(results: &Details) -> porter_core::GrantId {
    porter_core::GrantId::parse(&text_of(results, "grant").expect("a grant")).expect("grant id")
}

/// The D-Bus error name of a failed call.
pub fn error_name(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        zbus::Error::FDO(fdo) => {
            use zbus::DBusError;
            fdo.name().to_string()
        }
        other => format!("{other:?}"),
    }
}

pub fn refusal_name(refusal: Refusal) -> String {
    refusal_error_name(refusal)
}

pub const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";
pub const UNKNOWN_OBJECT: &str = "org.freedesktop.DBus.Error.UnknownObject";

/// Grants `photos` the storage account through the sheet and returns the grant.
pub async fn grant_photos(rig: &Rig) -> (zbus::Connection, porter_core::GrantId) {
    let client = rig.client("org.quire.Photos").await;
    let (code, results) = choose(&client).await;
    assert_eq!(code, 0, "{results:?}");
    let grant = grant_in(&results);
    (client, grant)
}

use ds_settings::live::LiveClient;
use ds_settings::schema::KeyPath;

/// A client of the settings module as `who`.
pub async fn settings_as(rig: &Rig, who: Caller) -> LiveClient {
    let connection = rig.client_as(who).await;
    LiveClient::new(
        &connection,
        "org.quire.Accounts1",
        &accountd::settings_path(),
    )
    .await
    .expect("client")
}

/// The Settings app's own client.
pub async fn settings(rig: &Rig) -> LiveClient {
    settings_as(rig, caller("org.quire.Settings", CallerRole::Settings)).await
}

pub fn key(path: &str) -> KeyPath {
    KeyPath(path.to_owned())
}

/// Collects the manager's signals a connection receives, by member name.
pub async fn listen(connection: &zbus::Connection) -> zbus::MessageStream {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.quire.Accounts1.Manager")
        .expect("interface")
        .build();
    zbus::MessageStream::for_match_rule(rule, connection, None)
        .await
        .expect("stream")
}

/// The member names received within `wait`. For proving that something does NOT arrive (a short
/// `wait` is right there) and for collecting what follows a signal already heard; a signal that
/// SHOULD arrive is waited for with [`heard_names`], which does not depend on the machine's speed.
pub async fn heard(stream: &mut zbus::MessageStream, wait: std::time::Duration) -> Vec<String> {
    let mut names = Vec::new();
    let deadline = tokio::time::Instant::now() + wait;
    while let Some(name) = next_member(stream, deadline).await {
        names.push(name);
    }
    names
}

/// The next signal's member name before `deadline`; `None` when time is up or the stream ends.
async fn next_member(
    stream: &mut zbus::MessageStream,
    deadline: tokio::time::Instant,
) -> Option<String> {
    use zbus::export::futures_core::Stream;
    loop {
        let next = tokio::time::timeout_at(
            deadline,
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)),
        )
        .await;
        match next {
            Ok(Some(Ok(message))) => {
                if let Some(member) = message.header().member() {
                    return Some(member.to_string());
                }
            }
            _ => return None,
        }
    }
}

/// Every member name received until each of `expected` has arrived (waited for by the clock,
/// [`porter_fake::Deadline`]: a signal that should come is not given up on because the machine
/// is slow), and then whatever else follows within a short settling window, so that a test can
/// still assert there was no extra or repeated signal. Fails the test naming the missing signals.
pub async fn heard_names(stream: &mut zbus::MessageStream, expected: &[&str]) -> Vec<String> {
    /// The window after the last expected signal in which extras are collected: a published
    /// change tells its listeners in one burst, so this proves "no more" and never decides
    /// whether a signal that should come does.
    const SETTLE: std::time::Duration = std::time::Duration::from_millis(300);
    let limit = porter_fake::Deadline::generous();
    let mut names: Vec<String> = Vec::new();
    while !expected.iter().all(|want| names.iter().any(|n| n == want)) {
        let left = porter_fake::GENEROUS.saturating_sub(limit.waited());
        match next_member(stream, tokio::time::Instant::now() + left).await {
            Some(name) => names.push(name),
            None => limit.fail(&format!("the signals {expected:?} (heard {names:?})")),
        }
    }
    names.extend(heard(stream, SETTLE).await);
    names
}

/// Every message on the bus, from the moment the tap is set: the bytes of each, as a monitor sees
/// them (file descriptors are not bytes, so what travels on one is not in them).
pub struct Tap(zbus::MessageStream);

impl Tap {
    pub async fn start(bus: &PrivateBus) -> Self {
        let monitor = bus.connect().await;
        zbus::fdo::MonitoringProxy::new(&monitor)
            .await
            .expect("proxy")
            .become_monitor(&[], 0)
            .await
            .expect("monitor");
        Self(zbus::MessageStream::from(monitor))
    }

    /// What has gone by so far.
    pub async fn drain(&mut self) -> Vec<Vec<u8>> {
        use zbus::export::futures_core::Stream;
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

/// Whether `needle` is anywhere in `haystack`.
pub fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}
