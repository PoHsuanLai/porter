//! The person's computers through the client (`DbusTransport::tailnet`), against the real accountd
//! front end on a private bus following a fake Tailscale on a scratch socket (never the real
//! one): the typed `Machine`s, the stream of changes, and the refusals read back as
//! `TailnetError`s. No dictionary is parsed here: that is what the reader is for.
#![cfg(feature = "dbus")]

use crate::common;

use accountd::{Options, TableCallers, TailnetWatch};
use common::bus::PrivateBus;
use porter_client::{DbusTransport, Machine, MachineOwner, TailnetError};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AppId, AppName, AuthKind, Isolation,
    ProviderId, Restriction,
};
use porter_dbus::{BusStream, Caller, CallerRole};
use porter_fake::{FixedClock, NOW, ScriptedSheets, cloud_provider};
use porter_fake_servers::{Daemon, FakeLocalApi, FakePeer, Network};
use porter_secrets::MemorySecrets;
use porter_service::{AccountService, Registry};
use porter_tailscale::LocalApi;
use std::sync::Arc;
use std::time::Duration;

struct World {
    bus: PrivateBus,
    callers: Arc<TableCallers>,
    fake: FakeLocalApi,
    _connection: zbus::Connection,
}

fn network() -> Network {
    Network::new(
        "ada@example.org",
        "ada@example.org",
        FakePeer::new("nSELF", "desk", 1, "100.64.0.1"),
    )
    .with_user(2, "bob@example.net")
    .with_peers(vec![
        FakePeer::new("nPI", "pi", 1, "100.64.0.2").with_ssh(),
        FakePeer::new("nOLD", "old-laptop", 1, "100.64.0.3").offline("2026-09-21T14:13:20Z"),
        FakePeer::new("nBUILD", "build-box", 1, "100.64.0.4").tagged("tag:ci"),
        FakePeer::new("nFRIEND", "friends-pc", 2, "100.64.0.5").shared_in(),
    ])
}

fn tailscale() -> Account {
    Account {
        id: AccountId::parse("tailscale").expect("id"),
        provider: ProviderId::parse("tailscale").expect("id"),
        label: AccountLabel("ada@example.org".into()),
        state: AccountState::Ok,
        auth: AuthKind::OwnProgram,
        capabilities: Vec::new(),
        restriction: Restriction::none(),
        endpoints: Vec::new(),
    }
}

impl World {
    async fn start(name: &str) -> Self {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("client-tailnet-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let fake = FakeLocalApi::start(&dir, Daemon::Running(network()))
            .await
            .expect("fake tailscale");
        let api = LocalApi::new(fake.socket()).with_program_dirs(Vec::new());
        let bus = PrivateBus::start();
        let callers = Arc::new(TableCallers::new());
        let connection = bus.connect().await;
        let service = Arc::new(AccountService::new(
            vec![cloud_provider()],
            Registry {
                accounts: vec![tailscale()],
                grants: vec![],
                toggles: vec![],
            },
            MemorySecrets::default(),
            ScriptedSheets::answering([]),
            FixedClock(NOW),
        ));
        let watch = TailnetWatch {
            api,
            backstop: Duration::from_millis(400),
            retry: Duration::from_millis(100),
            coalesce: Duration::from_millis(300),
        };
        accountd::serve_with(
            &connection,
            service,
            Arc::clone(&callers),
            Options {
                tailnet: Some(watch),
                ..Options::default()
            },
        )
        .await
        .expect("accountd serves");
        Self {
            bus,
            callers,
            fake,
            _connection: connection,
        }
    }

    async fn client(&self, role: CallerRole) -> zbus::Connection {
        let connection = self.bus.connect().await;
        let name = connection.unique_name().expect("name").to_string();
        self.callers.introduce_as(
            &name,
            Caller {
                app: AppId {
                    name: AppName::parse("org.quire.Temor").expect("app name"),
                    isolation: Isolation::Unsandboxed,
                },
                role,
            },
        );
        connection
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_terminal_reads_typed_machines_and_hears_of_changes() {
    let world = World::start("typed").await;
    let tailnet = DbusTransport::over(world.client(CallerRole::Terminal).await)
        .tailnet()
        .await
        .expect("tailnet");
    let mut changes = tailnet.watch().await.expect("watch");
    let machines: Vec<Machine> = tailnet.machines().await.expect("machines");
    let names: Vec<(&str, MachineOwner)> = machines
        .iter()
        .map(|m| (m.name.as_str(), m.owner))
        .collect();
    assert_eq!(
        names,
        [
            ("build-box", MachineOwner::Tagged),
            ("friends-pc", MachineOwner::Shared),
            ("old-laptop", MachineOwner::Mine),
            ("pi", MachineOwner::Mine),
        ]
    );
    let pi = machines.iter().find(|m| m.name == "pi").expect("pi");
    assert_eq!(pi.node.as_str(), "nPI");
    assert!(pi.ssh && pi.online);
    assert_eq!(pi.dns, "pi.tail1234.ts.net");

    world.fake.edit(|net| net.peers[0].online = false);
    let told = tokio::time::timeout(
        Duration::from_secs(4),
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut changes).poll_next(cx)),
    )
    .await
    .expect("told in time");
    assert_eq!(told, Some(()));
    let again = tailnet.machines().await.expect("machines");
    // The first of the fake's computers (`pi`) went offline.
    assert!(!again.iter().find(|m| m.name == "pi").expect("pi").online);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_and_a_stranger_are_refused_with_the_reason_read_back() {
    let world = World::start("refused").await;
    let app = DbusTransport::over(world.client(CallerRole::App).await)
        .tailnet()
        .await
        .expect("tailnet");
    assert!(matches!(app.machines().await, Err(TailnetError::Denied(_))));
    let stranger = DbusTransport::over(world.bus.connect().await)
        .tailnet()
        .await
        .expect("tailnet");
    assert!(matches!(
        stranger.machines().await,
        Err(TailnetError::Denied(_))
    ));
}
