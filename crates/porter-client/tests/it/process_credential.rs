//! A key for a process the launcher spawns, through `porter-client`'s `Launcher`, against the real
//! accountd front end on a private bus: issue by memfd and by tmpfs file, set a child up with it,
//! end it, and hear accountd end it when the grant goes.
#![cfg(feature = "dbus")]

use crate::common;

use accountd::{Options, SecretsDesk, TableCallers};
use common::bus::PrivateBus;
use common::eventually;
use porter_client::credential::{ChildKey, ChildKeyError, CredentialHandle, Delivery};
use porter_client::{Launcher, LauncherError};
use porter_core::audit::{CredentialEnd, Handoff};
use porter_core::capability::{AgentProgram, EnvName};
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::wire::Refusal;
use porter_core::{
    Account, AccountId, AppId, AppName, AuthKind, CapabilityKind, Credential, DataClass, GrantId,
    Isolation, SecretKey, SecretPurpose, SecretText, SpaceScope, UnixSeconds,
};
use porter_dbus::{BusStream, Caller, CallerRole, GrantsProxy};
use porter_fake::{FixedClock, NOW, RecordingAudit, ScriptedSheets, llm_account};
use porter_secrets::{MemorySecrets, Secrets, SecretsError};
use porter_service::{AccountService, Registry};
use std::io::Read;
use std::os::fd::AsFd;
use std::os::unix::fs::{FileExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const KEY: &str = "sk-ant-api03-S3CRET-CLIENT-KEY-0123456789";
const PROGRAM: &str = "claude-code";

/// Secrets the test can still read after they moved into the service and the desk.
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
    async fn delete_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        self.0.delete_account(account).await
    }
}

fn program() -> AgentProgram {
    AgentProgram::parse(PROGRAM).expect("program")
}

fn agent_app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Agent.claude-code").expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn grant(id: &str, app: AppId, scope: GrantScope) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("id"),
        key: GrantKey {
            app,
            account: AccountId::parse("anthropic").expect("id"),
            kind: CapabilityKind::Llm,
            class: DataClass::Prompt,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
        },
        decision: Decision::Allow,
        scope,
        at: UnixSeconds(1),
    }
}

fn gid(id: &str) -> GrantId {
    GrantId::parse(id).expect("id")
}

type Service = AccountService<
    porter_fake::FakeProvider,
    Shared,
    ScriptedSheets,
    FixedClock,
    porter_fake::MemoryStore,
    RecordingAudit,
>;

struct Rig {
    bus: PrivateBus,
    callers: Arc<TableCallers>,
    audit: RecordingAudit,
    runtime: PathBuf,
    _service: Arc<Service>,
    _daemon: zbus::Connection,
}

impl Rig {
    async fn start() -> Self {
        let bus = PrivateBus::start();
        let runtime = bus.scratch().join("runtime");
        std::fs::create_dir_all(&runtime).expect("runtime dir");
        let account = Account {
            id: AccountId::parse("anthropic").expect("id"),
            auth: AuthKind::ApiKey,
            ..llm_account()
        };
        let registry = Registry {
            accounts: vec![account],
            grants: vec![
                grant("g-always", agent_app(), GrantScope::Always),
                grant("g-once", agent_app(), GrantScope::Once),
                grant(
                    "g-companion",
                    AppId {
                        name: AppName::parse("org.quire.Companion").expect("name"),
                        isolation: Isolation::Flatpak,
                    },
                    GrantScope::Always,
                ),
            ],
            toggles: vec![],
        };
        let secrets = Shared::default();
        secrets
            .put(
                &SecretKey {
                    account: AccountId::parse("anthropic").expect("id"),
                    purpose: SecretPurpose::ApiKey,
                },
                &Credential::ApiKey(SecretText::new(KEY)),
            )
            .await
            .expect("key filed");
        let audit = RecordingAudit::default();
        let service: Arc<Service> = Arc::new(
            AccountService::new(
                Vec::new(),
                registry,
                secrets.clone(),
                ScriptedSheets::answering([]),
                FixedClock(NOW),
            )
            .with_store(porter_fake::MemoryStore::default())
            .with_audit(audit.clone()),
        );
        let callers = Arc::new(TableCallers::new());
        let daemon = bus.connect().await;
        let options = Options {
            keys: Some(Arc::new(SecretsDesk::new(
                secrets,
                audit.clone(),
                FixedClock(NOW),
            ))),
            runtime_dir: Some(runtime.clone()),
            ..Options::default()
        };
        accountd::serve_with(&daemon, Arc::clone(&service), Arc::clone(&callers), options)
            .await
            .expect("accountd serves");
        Self {
            bus,
            callers,
            audit,
            runtime,
            _service: service,
            _daemon: daemon,
        }
    }

    async fn as_role(&self, name: &str, role: CallerRole) -> zbus::Connection {
        let connection = self.bus.connect().await;
        let unique = connection.unique_name().expect("name").to_string();
        self.callers.introduce_as(
            &unique,
            Caller {
                app: AppId {
                    name: AppName::parse(name).expect("app name"),
                    isolation: Isolation::Unsandboxed,
                },
                role,
            },
        );
        connection
    }

    /// A launcher that registered claude-code, and the connection it lives on.
    async fn launcher(&self) -> (Launcher, zbus::Connection) {
        let connection = self
            .as_role("org.example.Launcher", CallerRole::AgentLauncher)
            .await;
        let launcher = Launcher::connect(&connection).await.expect("launcher");
        launcher.register(&[program()]).await.expect("registered");
        (launcher, connection)
    }
}

fn key_env() -> EnvName {
    EnvName::parse("ANTHROPIC_API_KEY").expect("var")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_memfd_credential_sets_a_child_up_by_value_or_by_an_inherited_descriptor() {
    let rig = Rig::start().await;
    let (launcher, _connection) = rig.launcher().await;
    let credential = launcher
        .issue_credential(&gid("g-always"), &program(), Handoff::Memfd)
        .await
        .expect("issued");
    assert_eq!(credential.handoff(), Handoff::Memfd);
    assert!(matches!(credential.handle(), CredentialHandle::Memfd(_)));

    // The key, read without moving the position the descriptor's holders share.
    assert_eq!(credential.read_key().expect("key").expose(), KEY);
    assert_eq!(credential.read_key().expect("again").expose(), KEY);
    if let CredentialHandle::Memfd(fd) = credential.handle() {
        let mut shared = std::fs::File::from(fd.as_fd().try_clone_to_owned().expect("dup"));
        let mut text = String::new();
        shared.read_to_string(&mut text).expect("a plain read");
        assert_eq!(text, KEY, "still at the start after read_key");
    }

    // By value: the variable the program declares, holding the key.
    match credential
        .child_key(&key_env(), Delivery::Value)
        .expect("by value")
    {
        ChildKey::Value { var, value } => {
            assert_eq!(var.as_str(), "ANTHROPIC_API_KEY");
            assert_eq!(value.expose(), KEY);
        }
        other => panic!("{other:?}"),
    }
    // By descriptor: `<VAR>_FILE` names /proc/self/fd/<n> and a duplicate is to be placed there.
    match credential
        .child_key(&key_env(), Delivery::File { child_fd: 7 })
        .expect("by descriptor")
    {
        ChildKey::File { var, path, inherit } => {
            assert_eq!(var.as_str(), "ANTHROPIC_API_KEY_FILE");
            assert_eq!(path, "/proc/self/fd/7");
            let inherit = inherit.expect("a descriptor to pass");
            assert_eq!(inherit.number, 7);
            let file = std::fs::File::from(inherit.fd);
            let mut bytes = vec![0u8; KEY.len()];
            file.read_exact_at(&mut bytes, 0).expect("read");
            assert_eq!(bytes, KEY.as_bytes());
        }
        other => panic!("{other:?}"),
    }

    // Ended by the launcher when the process is over; once.
    launcher
        .revoke_credential(credential.id())
        .await
        .expect("revoked");
    assert_eq!(
        launcher.revoke_credential(credential.id()).await,
        Err(LauncherError::UnknownCredential)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tmpfs_credential_is_a_private_file_that_ending_it_removes() {
    let rig = Rig::start().await;
    let (launcher, _connection) = rig.launcher().await;
    let credential = launcher
        .issue_credential(&gid("g-always"), &program(), Handoff::TmpfsFile)
        .await
        .expect("issued");
    let CredentialHandle::TmpfsFile(path) = credential.handle() else {
        panic!("a path");
    };
    assert!(path.starts_with(&rig.runtime));
    assert_eq!(
        std::fs::metadata(path).expect("file").permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(credential.read_key().expect("key").expose(), KEY);
    match credential
        .child_key(&key_env(), Delivery::File { child_fd: 3 })
        .expect("by file")
    {
        ChildKey::File {
            var,
            path: shown,
            inherit,
        } => {
            assert_eq!(var.as_str(), "ANTHROPIC_API_KEY_FILE");
            assert_eq!(shown, path.to_string_lossy());
            assert!(inherit.is_none(), "a path needs no descriptor");
        }
        other => panic!("{other:?}"),
    }
    launcher
        .revoke_credential(credential.id())
        .await
        .expect("revoked");
    assert!(!path.exists());
    // The audit has both ends, without the key.
    let audited = serde_json::to_string(&rig.audit.entries()).expect("json");
    assert!(audited.contains("process_credential_issued"), "{audited}");
    assert!(audited.contains("process_credential_revoked"), "{audited}");
    assert!(!audited.contains(KEY));
}

#[tokio::test(flavor = "multi_thread")]
async fn accountd_ending_a_credential_reaches_the_launcher_as_a_revocation() {
    let rig = Rig::start().await;
    let (launcher, _connection) = rig.launcher().await;
    let mut revocations = launcher.revocations().await.expect("stream");
    let file = launcher
        .issue_credential(&gid("g-always"), &program(), Handoff::TmpfsFile)
        .await
        .expect("issued");
    let CredentialHandle::TmpfsFile(path) = file.handle() else {
        panic!("a path");
    };
    let path = path.clone();

    // The app the grant is for withdraws it.
    let holder = rig
        .as_role("org.quire.Agent.claude-code", CallerRole::App)
        .await;
    GrantsProxy::new(&holder)
        .await
        .expect("proxy")
        .revoke("g-always")
        .await
        .expect("revoked");

    let told = tokio::time::timeout(
        Duration::from_secs(5),
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut revocations).poll_next(cx)),
    )
    .await
    .expect("in time")
    .expect("open")
    .expect("well formed");
    assert_eq!(&told.id, file.id());
    assert_eq!(told.reason, CredentialEnd::GrantRevoked);
    eventually("the file to go", || !path.exists()).await;
    // It is over: the launcher's own revoke has nothing left to end.
    assert_eq!(
        launcher.revoke_credential(file.id()).await,
        Err(LauncherError::UnknownCredential)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_is_a_typed_error_and_a_key_is_never_part_of_it() {
    let rig = Rig::start().await;
    let (launcher, _connection) = rig.launcher().await;
    let issue = |grant: &str, way| {
        let (launcher, grant) = (launcher.clone(), gid(grant));
        async move { launcher.issue_credential(&grant, &program(), way).await }
    };
    assert!(matches!(
        issue("g-once", Handoff::Memfd).await,
        Err(LauncherError::OnceGrant)
    ));
    assert!(matches!(
        issue("g-companion", Handoff::Memfd).await,
        Err(LauncherError::Refused(Refusal::AudienceNotGranted))
    ));
    assert!(matches!(
        issue("g-404", Handoff::TmpfsFile).await,
        Err(LauncherError::Refused(Refusal::UnknownGrant))
    ));
    // A program this connection did not register.
    assert!(matches!(
        launcher
            .issue_credential(
                &gid("g-always"),
                &AgentProgram::parse("codex").expect("program"),
                Handoff::Memfd
            )
            .await,
        Err(LauncherError::NotRegistered)
    ));
    // Not the launcher: an app is denied by accountd.
    let app = rig.as_role("org.example.App", CallerRole::App).await;
    let impostor = Launcher::connect(&app).await.expect("proxy");
    assert!(matches!(
        impostor
            .issue_credential(&gid("g-always"), &program(), Handoff::Memfd)
            .await,
        Err(LauncherError::Denied(_))
    ));
    assert!(matches!(
        impostor
            .revoke_credential(&porter_core::ProcessCredentialId::parse("cred-1").expect("id"))
            .await,
        Err(LauncherError::Denied(_))
    ));
    let audited = serde_json::to_string(&rig.audit.entries()).expect("json");
    assert!(!audited.contains("process_credential"), "{audited}");
}

#[test]
fn a_child_key_error_names_what_went_wrong_without_the_key() {
    let text = ChildKeyError::NotText.to_string();
    assert!(!text.contains("sk-"));
}
