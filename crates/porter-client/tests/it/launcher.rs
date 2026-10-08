//! The launcher's side of an agent's login, against the real accountd front end on a private bus:
//! register, a stream of requests, report. The sheet is scripted to close at once (the person
//! walks away), which leaves the request pending, as accountd keeps it: the launcher still gets
//! it and the account still follows its report. The wait, the outcomes on the sheet and the
//! expiry are in accountd's own `agent_launcher` tests.
#![cfg(feature = "dbus")]

use crate::common;

use accountd::TableCallers;
use common::bus::PrivateBus;
use ds_settings::live::LiveClient;
use ds_settings::schema::KeyPath;
use porter_client::{AskKind, Launcher, LauncherError};
use porter_core::capability::{AgentCap, AgentProgram};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AppId, AppName, AuthKind, Capability, Claim,
    Isolation, LoginFault, LoginOutcome, LoginRequestId, Offer, Provenance, ProviderId,
    Restriction, Subject,
};
use porter_dbus::{
    AccountProxy, BusStream, Caller, CallerRole, Details, account_path, zvariant::ObjectPath,
};
use porter_fake::{FixedClock, NOW, ScriptedSheets};
use porter_secrets::MemorySecrets;
use porter_service::{AccountService, Registry};
use std::sync::Arc;
use std::time::Duration;

type Service = AccountService<porter_fake::FakeProvider, MemorySecrets, ScriptedSheets, FixedClock>;

fn program(name: &str) -> AgentProgram {
    AgentProgram::parse(name).expect("program")
}

fn agent(name: &str) -> Account {
    let cap = AgentCap {
        program: program(name),
        key_env: None,
        base_url_env: None,
        protocols: Default::default(),
    };
    Account {
        id: AccountId::parse(name).expect("id"),
        provider: ProviderId::parse(name).expect("provider"),
        label: AccountLabel(name.to_owned()),
        state: AccountState::NeedsLogin,
        auth: AuthKind::AgentLogin,
        capabilities: vec![Claim {
            subject: Subject::Agent(program(name)),
            offer: Offer::Present(Capability::Agent(Box::new(cap))),
            provenance: Provenance::Declared,
        }],
        restriction: Restriction::none(),
        endpoints: Vec::new(),
    }
}

struct Rig {
    bus: PrivateBus,
    callers: Arc<TableCallers>,
    service: Arc<Service>,
    _daemon: zbus::Connection,
}

impl Rig {
    async fn start() -> Self {
        let bus = PrivateBus::start();
        let registry = Registry {
            accounts: vec![agent("claude-code")],
            grants: vec![],
            toggles: vec![],
        };
        // One conversation that is closed as soon as it opens, for the one sign-in a test asks.
        let sheets = ScriptedSheets::answering([]).conversing([Vec::new()]);
        let service = Arc::new(AccountService::new(
            Vec::new(),
            registry,
            MemorySecrets::default(),
            sheets,
            FixedClock(NOW),
        ));
        let callers = Arc::new(TableCallers::new());
        let daemon = bus.connect().await;
        accountd::serve(&daemon, Arc::clone(&service), Arc::clone(&callers))
            .await
            .expect("accountd serves");
        Self {
            bus,
            callers,
            service,
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

    fn state(&self) -> AccountState {
        self.service.registry().accounts[0].state
    }
}

async fn next<S: BusStream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_launcher_registers_hears_a_login_and_reports_it() {
    let rig = Rig::start().await;
    let connection = rig
        .as_role("org.example.Launcher", CallerRole::AgentLauncher)
        .await;
    let launcher = Launcher::connect(&connection).await.expect("launcher");
    let mut requests = launcher.requests().await.expect("requests");
    launcher
        .register(&[program("claude-code")])
        .await
        .expect("registered");

    // The shell asks for the sign-in; the sheet is closed at once and the request stays.
    let shell = rig.as_role("org.quire.Shell", CallerRole::SheetHost).await;
    let account = AccountProxy::builder(&shell)
        .path(
            ObjectPath::try_from(account_path(&AccountId::parse("claude-code").expect("id")))
                .expect("path")
                .into_owned(),
        )
        .expect("path")
        .build()
        .await
        .expect("account");
    account
        .reauthenticate("", &Details::new())
        .await
        .expect("a request");

    let ask = tokio::time::timeout(Duration::from_secs(5), next(&mut requests))
        .await
        .expect("a request in time")
        .expect("open stream")
        .expect("well formed");
    assert_eq!(ask.kind, AskKind::Login);
    assert_eq!(ask.account.as_str(), "claude-code");
    assert_eq!(ask.program, program("claude-code"));
    assert_eq!(rig.state(), AccountState::NeedsLogin);

    launcher
        .report_login(&ask.request, LoginOutcome::Ready)
        .await
        .expect("reported");
    assert_eq!(rig.state(), AccountState::Ok);
    // Said once.
    assert_eq!(
        launcher
            .report_login(&ask.request, LoginOutcome::Ready)
            .await,
        Err(LauncherError::UnknownRequest)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_has_one_launcher_and_only_the_launcher_may_use_the_calls() {
    let rig = Rig::start().await;
    let first = rig
        .as_role("org.example.First", CallerRole::AgentLauncher)
        .await;
    let second = rig
        .as_role("org.example.Second", CallerRole::AgentLauncher)
        .await;
    let first = Launcher::connect(&first).await.expect("launcher");
    let second = Launcher::connect(&second).await.expect("launcher");
    first
        .register(&[program("claude-code")])
        .await
        .expect("registered");
    first
        .register(&[program("claude-code")])
        .await
        .expect("again is a no-op");
    assert_eq!(
        second
            .register(&[program("codex"), program("claude-code")])
            .await,
        Err(LauncherError::AlreadyRegistered)
    );
    second
        .register(&[program("codex")])
        .await
        .expect("codex was left free");

    for role in [
        CallerRole::App,
        CallerRole::PorterDaemon,
        CallerRole::Settings,
    ] {
        let connection = rig.as_role("org.example.Other", role).await;
        let launcher = Launcher::connect(&connection).await.expect("launcher");
        let refused = launcher.register(&[program("aider")]).await;
        assert!(
            matches!(refused, Err(LauncherError::Denied(_))),
            "{role:?}: {refused:?}"
        );
        let refused = launcher
            .report_login(
                &LoginRequestId::parse("login-1").expect("id"),
                LoginOutcome::Ready,
            )
            .await;
        assert!(
            matches!(refused, Err(LauncherError::Denied(_))),
            "{role:?}: {refused:?}"
        );
    }
    let stranger = rig.bus.connect().await;
    let launcher = Launcher::connect(&stranger).await.expect("launcher");
    assert!(matches!(
        launcher.register(&[program("aider")]).await,
        Err(LauncherError::Denied(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_logout_request_reaches_the_launcher_and_its_report_signs_the_agent_out() {
    let rig = Rig::start().await;
    let connection = rig
        .as_role("org.example.Launcher", CallerRole::AgentLauncher)
        .await;
    let launcher = Launcher::connect(&connection).await.expect("launcher");
    let mut requests = launcher.requests().await.expect("requests");
    launcher
        .register(&[program("claude-code")])
        .await
        .expect("registered");
    // Signed in, then signed out from Settings.
    rig.service
        .set_agent_state(
            &AccountId::parse("claude-code").expect("id"),
            porter_core::AgentState::Ready,
        )
        .await
        .expect("ready");
    let settings = rig
        .as_role("org.quire.Settings", CallerRole::Settings)
        .await;
    let client = LiveClient::new(&settings, "org.quire.Accounts1", &accountd::settings_path())
        .await
        .expect("client");
    client
        .invoke(&KeyPath("accounts.claude-code.sign_out".to_owned()))
        .await
        .expect("asked");

    let ask = tokio::time::timeout(Duration::from_secs(5), next(&mut requests))
        .await
        .expect("a request in time")
        .expect("open stream")
        .expect("well formed");
    assert_eq!(ask.kind, AskKind::Logout);
    assert_eq!(rig.state(), AccountState::Ok);
    // A login report for a logout request is no request.
    assert_eq!(
        launcher
            .report_login(&ask.request, LoginOutcome::Ready)
            .await,
        Err(LauncherError::UnknownRequest)
    );
    launcher
        .report_logout(&ask.request, LoginOutcome::Failed(LoginFault::Other))
        .await
        .expect("reported");
    assert_eq!(rig.state(), AccountState::Ok);

    client
        .invoke(&KeyPath("accounts.claude-code.sign_out".to_owned()))
        .await
        .expect("asked again");
    let ask = tokio::time::timeout(Duration::from_secs(5), next(&mut requests))
        .await
        .expect("a request in time")
        .expect("open stream")
        .expect("well formed");
    launcher
        .report_logout(&ask.request, LoginOutcome::Ready)
        .await
        .expect("reported");
    assert_eq!(rig.state(), AccountState::NeedsLogin);
}
