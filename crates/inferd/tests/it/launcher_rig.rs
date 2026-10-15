//! The rig the launcher's tests share: `serve_launcher` on a private bus over a hosted
//! `AccountService` (memory secrets, a scripted consent sheet, a recording audit), and fake
//! launcher connections the caller table knows as the agent launcher. No test touches the real
//! bus or keyring.

use crate::hosting::bus::PrivateBus;

use inferd::launcher::{LauncherConfig, Launchers, LoginTiming, serve_launcher};
use inferd::peers::{Caller, Role, TablePeers};
use porter_core::clock::FixedClock;
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::{
    Account, AccountId, AccountState, AppId, AppName, AuthKind, CapabilityKind, Credential,
    DataClass, GrantId, Isolation, SecretKey, SecretPurpose, SecretText, SpaceScope, UnixSeconds,
};
use porter_dbus::LauncherProxy;
use porter_fake::{
    FakeProvider, MemoryStore, RecordingAudit, Scripted, ScriptedSheets, llm_account, llm_provider,
};
use porter_secrets::{MemorySecrets, Secrets, SecretsError};
use porter_service::{AccountService, Registry};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

pub const KEY: &str = "sk-ant-api03-S3CRET-LAUNCHER-KEY-0123456789";
pub const PROGRAM: &str = "claude-code";
/// The account that holds an API key, and the one that does not.
pub const KEYED: &str = "anthropic";
pub const OAUTH: &str = "oauth-llm";
/// The agent account the launcher signs in.
pub const AGENT: &str = "claude-code";

/// The secrets, shared between the service and the test.
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

pub type Svc =
    AccountService<FakeProvider, Shared, ScriptedSheets, FixedClock, MemoryStore, RecordingAudit>;

/// A runtime directory of its own, removed on drop.
pub struct Runtime(pub PathBuf);

impl Runtime {
    pub fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = porter_fake::scratch_dir(&format!(
            "launcher-runtime-{}",
            N.fetch_add(1, Ordering::Relaxed)
        ));
        Self(dir)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn account(id: &str, auth: AuthKind) -> Account {
    Account {
        id: AccountId::parse(id).expect("id"),
        auth,
        ..llm_account()
    }
}

pub fn agent_app(program: &str) -> AppId {
    AppId {
        name: AppName::parse(&format!("org.quire.Agent.{program}")).expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

/// A permit: an `Allow` (or `Deny`) of `account`'s key to `app`.
pub fn permit(id: &str, app: AppId, account: &str, scope: GrantScope, decision: Decision) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("id"),
        key: GrantKey {
            app,
            account: AccountId::parse(account).expect("id"),
            kind: CapabilityKind::Llm,
            class: DataClass::Prompt,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
        },
        decision,
        scope,
        at: UnixSeconds(1),
    }
}

/// The launcher as the tests start it.
pub struct Rig {
    pub bus: PrivateBus,
    pub peers: Arc<TablePeers>,
    pub service: Arc<Svc>,
    pub audit: RecordingAudit,
    pub sheets: porter_fake::AskLog,
    pub launchers: Launchers,
    pub runtime: Runtime,
    _connection: zbus::Connection,
}

impl Rig {
    /// Serving, with the two Llm accounts and the agent's account, `permits` already given, and
    /// `consent` as the person's answers to the key sheet.
    pub async fn start(permits: Vec<Grant>, consent: Vec<Scripted>, timing: LoginTiming) -> Self {
        let bus = PrivateBus::start();
        let connection = bus.connect().await;
        let secrets = Shared::default();
        secrets
            .put(
                &SecretKey {
                    account: AccountId::parse(KEYED).expect("id"),
                    purpose: SecretPurpose::ApiKey,
                },
                &Credential::ApiKey(SecretText::new(KEY)),
            )
            .await
            .expect("key filed");
        let registry = Registry {
            accounts: vec![
                account(KEYED, AuthKind::ApiKey),
                account(OAUTH, AuthKind::OAuthPkce),
                Account {
                    state: AccountState::NeedsLogin,
                    ..account(AGENT, AuthKind::AgentLogin)
                },
            ],
            grants: permits,
            toggles: vec![],
        };
        let sheets = ScriptedSheets::answering(consent);
        let log = sheets.log();
        let audit = RecordingAudit::default();
        let service = Arc::new(
            AccountService::new(
                vec![llm_provider()],
                registry,
                secrets.clone(),
                sheets,
                FixedClock(porter_fake::NOW),
            )
            .with_store(MemoryStore::default())
            .with_audit(audit.clone()),
        );
        let peers = Arc::new(TablePeers::new());
        let runtime = Runtime::new();
        let config = LauncherConfig::new(runtime.0.clone())
            .with_timing(timing)
            .with_audit(Arc::new(audit.clone()));
        let launchers = serve_launcher(
            &connection,
            Arc::clone(&service),
            Arc::clone(&peers),
            config,
        )
        .await
        .expect("the launcher serves");
        connection
            .request_name(porter_dbus::INFERENCE_BUS)
            .await
            .expect("the name");
        Self {
            bus,
            peers,
            service,
            audit,
            sheets: log,
            launchers,
            runtime,
            _connection: connection,
        }
    }

    /// A new connection the table knows as `role`.
    pub async fn client_as(&self, name: &str, role: Role) -> zbus::Connection {
        let connection = self.bus.connect().await;
        self.peers.introduce(
            connection.unique_name().expect("name").as_str(),
            Caller {
                app: AppId {
                    name: AppName::parse(name).expect("app name"),
                    isolation: Isolation::Flatpak,
                },
                role,
            },
        );
        connection
    }

    /// A fake launcher process.
    pub async fn launcher(&self, name: &str) -> Launcher {
        let connection = self.client_as(name, Role::AgentLauncher).await;
        let proxy = launcher_proxy(&connection).await;
        Launcher { connection, proxy }
    }
}

/// The launcher interface on inferd's name (the proxy's default service, taken by the rig).
async fn launcher_proxy(connection: &zbus::Connection) -> LauncherProxy<'static> {
    LauncherProxy::new(connection).await.expect("proxy")
}

/// A launcher connection and its proxy.
pub struct Launcher {
    pub connection: zbus::Connection,
    pub proxy: LauncherProxy<'static>,
}

/// A timing that expires at once on a fixed clock moved by the test: the bound is short and
/// looked at often.
pub fn quick_timing() -> LoginTiming {
    LoginTiming::new().with_bound(Duration::from_secs(30), Duration::from_millis(20))
}
