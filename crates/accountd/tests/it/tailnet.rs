//! Tailscale through accountd on a private bus: the real Tailnet family and accountd's watch of
//! Tailscale over a fake LocalAPI on a scratch socket (never the real tailscaled), a sheet host
//! written by hand that does what a person would, and clients introduced by role. Adding the
//! account (running, signed out, every refusal), the account's state following Tailscale, and
//! `org.quire.Tailnet1`: the rows for every kind of computer, `Changed` once for a burst, an
//! empty list with no account, and who may ask.

use crate::common;

use accountd::{BusSheets, Options, TableCallers, TailnetWatch, serve_with};
use common::bus::PrivateBus;
use common::host::{HostLog, SheetHost, send_input};
use common::{Shared, caller, error_name, eventually};
use porter_core::sheet::{SheetInput, SheetView, SignInFault};
use porter_core::{
    Account, AccountLabel, AccountState, AuthKind, Machine, MachineOwner, ProviderId, Restriction,
};
use porter_dbus::{
    BusStream, CallerRole, ManagerProxy, Sheet, SpacesProxy, TailnetProxy, machine_from_dbus,
};
use porter_fake::{Deadline, FixedClock, MemoryStore, RecordingAudit};
use porter_fake_servers::{Daemon, FakeLocalApi, FakePeer, Network};
use porter_families::{FamilyProvider, TailnetProvider};
use porter_http::{SharedSleep, Sleep};
use porter_service::{AccountService, Registry};
use porter_tailscale::LocalApi;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LOGIN_PAGE: &str = "https://login.tailscale.com/a/0123456789abcdef";

type Svc = AccountService<
    FamilyProvider,
    Shared,
    BusSheets<TableCallers>,
    FixedClock,
    MemoryStore,
    RecordingAudit,
>;

/// Waits a moment instead of the seconds the family asks for, so a sign-in that polls finishes.
#[derive(Debug, Clone, Copy)]
struct Quick;

impl Sleep for Quick {
    async fn sleep(&self, _how_long: Duration) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("tailnet-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

/// A small network: this computer (`desk`), and a computer of every kind.
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

fn tailscale_account(state: AccountState) -> Account {
    Account {
        id: porter_core::AccountId::parse("tailscale").expect("id"),
        provider: ProviderId::parse("tailscale").expect("id"),
        label: AccountLabel("ada@example.org".into()),
        state,
        auth: AuthKind::OwnProgram,
        capabilities: Vec::new(),
        restriction: Restriction::none(),
        endpoints: Vec::new(),
    }
}

/// How the fake's world looks to a test.
struct Scene {
    /// The fake LocalAPI, when there is one.
    fake: Option<FakeLocalApi>,
    /// The client of its socket.
    api: LocalApi,
    accounts: Vec<Account>,
}

impl Scene {
    /// A fake in `daemon`'s state, and the accounts already added.
    async fn with(name: &str, daemon: Daemon, accounts: Vec<Account>) -> Self {
        let fake = FakeLocalApi::start(&scratch(name), daemon)
            .await
            .expect("fake tailscale");
        let api = LocalApi::new(fake.socket()).with_program_dirs(Vec::new());
        Self {
            fake: Some(fake),
            api,
            accounts,
        }
    }

    fn fake(&self) -> &FakeLocalApi {
        self.fake.as_ref().expect("a fake")
    }
}

/// Who watches, and how fast, for the tests.
fn quick_watch(api: LocalApi) -> TailnetWatch {
    TailnetWatch {
        api,
        backstop: Duration::from_millis(400),
        retry: Duration::from_millis(100),
        coalesce: Duration::from_millis(300),
    }
}

struct World {
    bus: PrivateBus,
    callers: Arc<TableCallers>,
    service: Arc<Svc>,
    scene: Scene,
    host_connection: zbus::Connection,
    host_log: HostLog,
    _connection: zbus::Connection,
    cursor: Mutex<(usize, usize)>,
}

impl World {
    async fn start(scene: Scene, watching: bool) -> Self {
        let spec = porter_provider::shipped_specs()
            .into_iter()
            .find(|s| s.id.as_str() == "tailscale")
            .expect("tailscale ships");
        let providers = vec![FamilyProvider::Tailnet(TailnetProvider::new(
            spec,
            scene.api.clone(),
            SharedSleep::new(Quick),
        ))];
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
        let registry = Registry {
            accounts: scene.accounts.clone(),
            grants: vec![],
            toggles: vec![],
        };
        let sheets = BusSheets::new(connection.clone(), Arc::clone(&callers));
        let secrets = Shared::default();
        let service = Arc::new(
            AccountService::new(
                providers,
                registry,
                secrets,
                sheets,
                FixedClock(porter_fake::NOW),
            )
            .with_store(MemoryStore::default())
            .with_audit(RecordingAudit::default()),
        );
        serve_with(
            &connection,
            Arc::clone(&service),
            Arc::clone(&callers),
            Options {
                tailnet: watching.then(|| quick_watch(scene.api.clone())),
                ..Options::default()
            },
        )
        .await
        .expect("accountd serves");
        Self {
            bus,
            callers,
            service,
            scene,
            host_connection,
            host_log,
            _connection: connection,
            cursor: Mutex::new((0, 0)),
        }
    }

    /// A connection accountd knows as `name` in `role`.
    async fn client(&self, name: &str, role: CallerRole) -> zbus::Connection {
        let connection = self.bus.connect().await;
        let unique = connection.unique_name().expect("name").to_string();
        self.callers.introduce_as(&unique, caller(name, role));
        connection
    }

    /// Plays the person at the sheet host until `done` resolves: confirms the review, dismisses a
    /// failure after noting why, and tells `on_page` the page of a browser step.
    async fn person<T>(
        &self,
        failed: &Mutex<Vec<SignInFault>>,
        on_page: &dyn Fn(&str),
        done: impl std::future::Future<Output = T>,
    ) -> T {
        tokio::pin!(done);
        let (mut opened, mut updated) = *self.cursor.lock().expect("cursor");
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
                    SheetView::BrowserWait { url, .. } => {
                        on_page(url.as_str());
                        None
                    }
                    SheetView::Review(_) => Some(SheetInput::Confirm(Vec::new())),
                    SheetView::Failed { fault, .. } => {
                        failed.lock().expect("faults").push(fault);
                        Some(SheetInput::Dismiss)
                    }
                    _ => None,
                };
                if let Some(input) = input {
                    send_input(&self.host_connection, &handle, &input).await;
                }
            }
        }
    }

    /// Adds Tailscale as an app asks to, and answers the code and the faults the person saw.
    async fn add(&self, on_page: &dyn Fn(&str)) -> (u32, Vec<SignInFault>) {
        let app = self.client("org.example.Files", CallerRole::App).await;
        let mut sheet = Sheet::subscribe(&app).await.expect("subscribe");
        let path = ManagerProxy::new(&app)
            .await
            .expect("proxy")
            .add_account("tailscale", "", &sheet.options())
            .await
            .expect("add_account");
        let failed = Mutex::new(Vec::new());
        let (code, _results) = self
            .person(&failed, on_page, sheet.response(&path))
            .await
            .expect("response");
        let faults = failed.into_inner().expect("faults");
        (code, faults)
    }

    fn state(&self) -> Option<AccountState> {
        self.service
            .registry()
            .accounts
            .iter()
            .find(|a| a.auth == AuthKind::OwnProgram)
            .map(|a| a.state)
    }

    async fn machines_as(&self, role: CallerRole) -> Result<Vec<Machine>, zbus::Error> {
        let connection = self.client("org.example.Reader", role).await;
        read(&connection).await
    }
}

async fn read(connection: &zbus::Connection) -> Result<Vec<Machine>, zbus::Error> {
    let rows = TailnetProxy::new(connection).await?.machines().await?;
    Ok(rows
        .iter()
        .map(|row| machine_from_dbus(row).expect("a well formed row"))
        .collect())
}

/// Waits for the next `Changed`, as long as a starved machine needs (`Deadline::generous`): a
/// signal that should come is waited for, never raced against a short timer.
async fn told(stream: &mut porter_dbus::TailnetChangedStream, what: &str) {
    let deadline = Deadline::generous();
    let next = std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx));
    match tokio::time::timeout(porter_fake::GENEROUS, next).await {
        Ok(Some(_)) => {}
        _ => deadline.fail(what),
    }
}

/// Whether NO `Changed` arrives in `window`. This is the one short wait here, and it is for
/// something that must not happen: it can only miss a signal that is late, and so passes when it
/// should fail on a starved machine, never fails when it should pass. The windows used are
/// several times the coalescing window of the tests (300 ms), the time a second signal for the
/// same burst would come in.
async fn quiet_for(stream: &mut porter_dbus::TailnetChangedStream, window: Duration) -> bool {
    tokio::time::timeout(
        window,
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)),
    )
    .await
    .is_err()
}

/// Waits until nothing listens on the fake's socket any more (its server was stopped).
async fn until_stopped(api: &LocalApi) {
    let deadline = Deadline::generous();
    while api.status().await != Err(porter_tailscale::TailscaleError::NotRunning) {
        if deadline.passed() {
            deadline.fail("the stopped fake to refuse connections");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---- adding it ----

#[tokio::test(flavor = "multi_thread")]
async fn adding_tailscale_when_it_is_running_files_an_account_that_holds_nothing() {
    let scene = Scene::with("add-running", Daemon::Running(network()), vec![]).await;
    let world = World::start(scene, false).await;
    let (code, faults) = world.add(&|_| {}).await;
    assert_eq!((code, faults.as_slice()), (0, &[][..]));
    let registry = world.service.registry();
    let [account] = registry.accounts.as_slice() else {
        panic!("one account: {:?}", registry.accounts);
    };
    assert_eq!(account.auth, AuthKind::OwnProgram);
    assert_eq!(account.state, AccountState::Ok);
    assert_eq!(account.label.0, "ada@example.org");
    assert!(account.capabilities.is_empty() && account.endpoints.is_empty());
    // It asked Tailscale who is signed in, and nothing else: no sign-in was started.
    assert_eq!(world.scene.fake().logins(), 0);
    assert!(
        world
            .scene
            .fake()
            .requests()
            .iter()
            .all(|r| r.starts_with("GET /localapi/v0/status")),
        "{:?}",
        world.scene.fake().requests()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn adding_tailscale_when_it_is_signed_out_hands_over_its_own_page_and_waits() {
    let scene = Scene::with(
        "add-signed-out",
        Daemon::SignedOut {
            auth_url: LOGIN_PAGE.to_owned(),
        },
        vec![],
    )
    .await;
    let world = World::start(scene, false).await;
    let pages = Mutex::new(Vec::new());
    let fake = world.scene.fake().handle();
    let (code, faults) = world
        .add(&|page| {
            pages.lock().expect("pages").push(page.to_owned());
            // The person signs in on Tailscale's page: Tailscale is running and signed in.
            fake.set(Daemon::Running(network()));
        })
        .await;
    assert_eq!((code, faults.as_slice()), (0, &[][..]));
    assert_eq!(*pages.lock().expect("pages"), [LOGIN_PAGE]);
    assert_eq!(
        world.scene.fake().logins(),
        1,
        "Tailscale was asked to sign in"
    );
    let registry = world.service.registry();
    assert_eq!(registry.accounts.len(), 1);
    assert_eq!(registry.accounts[0].state, AccountState::Ok);
    assert_eq!(registry.accounts[0].label.0, "ada@example.org");
}

#[tokio::test(flavor = "multi_thread")]
async fn adding_tailscale_says_in_closed_words_why_it_could_not() {
    // (name, how the world is, the fault the person is shown)
    let signed_out = |url: &str| Daemon::SignedOut {
        auth_url: url.to_owned(),
    };
    let cases: Vec<(&str, Daemon, SignInFault)> = vec![
        ("refusing", Daemon::Refusing, SignInFault::NotAllowed),
        ("starting", Daemon::Changing, SignInFault::NotRunning),
        ("no-page", signed_out(""), SignInFault::SignedOut),
    ];
    for (name, daemon, want) in cases {
        let scene = Scene::with(&format!("refuse-{name}"), daemon, vec![]).await;
        let world = World::start(scene, false).await;
        let (code, faults) = world.add(&|_| {}).await;
        assert_eq!(faults, [want], "{name}");
        assert_ne!(code, 0, "{name}");
        assert!(world.service.registry().accounts.is_empty(), "{name}");
    }

    // A user who may read Tailscale but not sign it in cannot be handed its page.
    let scene = Scene::with("refuse-read-only", signed_out(LOGIN_PAGE), vec![]).await;
    scene.fake().set_read_only(true);
    let world = World::start(scene, false).await;
    let (_, faults) = world.add(&|_| {}).await;
    assert_eq!(faults, [SignInFault::NotAllowed]);
    assert_eq!(world.scene.fake().logins(), 0);

    // Stopped: the socket is left behind and nobody answers on it.
    let scene = Scene::with("refuse-stopped", Daemon::Running(network()), vec![]).await;
    scene.fake().stop();
    until_stopped(&scene.api).await;
    let world = World::start(scene, false).await;
    let (_, faults) = world.add(&|_| {}).await;
    assert_eq!(faults, [SignInFault::NotRunning]);

    // No socket and no program: not installed. The program there and no socket: not running.
    let dir = scratch("refuse-not-installed");
    let missing = dir.join("tailscaled.sock");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    for (installed, want) in [
        (false, SignInFault::NotInstalled),
        (true, SignInFault::NotRunning),
    ] {
        if installed {
            std::fs::write(bin.join("tailscaled"), b"#!/bin/sh\n").expect("program");
        }
        let scene = Scene {
            fake: None,
            api: LocalApi::new(&missing).with_program_dirs(vec![bin.clone()]),
            accounts: vec![],
        };
        let world = World::start(scene, false).await;
        let (_, faults) = world.add(&|_| {}).await;
        assert_eq!(faults, [want], "installed: {installed}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn adding_tailscale_twice_is_the_same_account() {
    let scene = Scene::with(
        "add-twice",
        Daemon::Running(network()),
        vec![tailscale_account(AccountState::Ok)],
    )
    .await;
    let world = World::start(scene, false).await;
    let (_, faults) = world.add(&|_| {}).await;
    assert_eq!(faults, [SignInFault::AlreadyAdded]);
    assert_eq!(world.service.registry().accounts.len(), 1);
}

// ---- its state follows Tailscale ----

#[tokio::test(flavor = "multi_thread")]
async fn the_account_follows_tailscale_while_it_is_added() {
    let scene = Scene::with(
        "follows",
        Daemon::Running(network()),
        vec![tailscale_account(AccountState::Offline)],
    )
    .await;
    let world = World::start(scene, true).await;
    let fake = world.scene.fake().handle();
    // Running and signed in: working, at once.
    eventually("the account to be working", || {
        world.state() == Some(AccountState::Ok)
    })
    .await;
    // In the middle of starting, nothing is concluded. This is a check that something does NOT
    // happen, so it is a short window: the 700 ms is longer than the look behind the stream
    // (400 ms here) plus the retry (100 ms), so a wrong conclusion would have been drawn by then.
    fake.set(Daemon::Changing);
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(world.state(), Some(AccountState::Ok));
    // Signed out: the state that means "sign in again".
    fake.set(Daemon::SignedOut {
        auth_url: LOGIN_PAGE.to_owned(),
    });
    eventually("the account to need a sign in", || {
        world.state() == Some(AccountState::NeedsLogin)
    })
    .await;
    // Signed in again, in Tailscale: working, with no sign-in through porter.
    fake.set(Daemon::Running(network()));
    eventually("the account to work again", || {
        world.state() == Some(AccountState::Ok)
    })
    .await;
    // Tailscale stops: off, and the account stays.
    world.scene.fake().stop();
    eventually("the account to be off", || {
        world.state() == Some(AccountState::Offline)
    })
    .await;
    // It comes back: working again, without anyone touching the account.
    world.scene.fake().restart().await.expect("restart");
    eventually("the account to work after a restart", || {
        world.state() == Some(AccountState::Ok)
    })
    .await;
    assert_eq!(world.service.registry().accounts.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_the_watch_did_not_announce_is_found_by_the_look_behind_it() {
    let scene = Scene::with(
        "backstop",
        Daemon::Running(network()),
        vec![tailscale_account(AccountState::Ok)],
    )
    .await;
    let world = World::start(scene, true).await;
    let reader = world
        .client("org.example.Reader", CallerRole::Terminal)
        .await;
    let proxy = TailnetProxy::new(&reader).await.expect("proxy");
    let mut changes = proxy.receive_changed().await.expect("subscribe");
    read(&reader).await.expect("machines");
    // The stream says nothing of this; the half-minute look (here 400 ms) does.
    world
        .scene
        .fake()
        .edit_quietly(|net| net.peers[0].online = false);
    told(&mut changes, "a Changed for a change the stream missed").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_the_account_does_not_sign_tailscale_out() {
    let scene = Scene::with(
        "remove",
        Daemon::Running(network()),
        vec![tailscale_account(AccountState::Ok)],
    )
    .await;
    let world = World::start(scene, false).await;
    let id = porter_core::AccountId::parse("tailscale").expect("id");
    world
        .service
        .remove_with_revoke(&id)
        .await
        .expect("removed");
    assert!(world.service.registry().accounts.is_empty());
    // Porter asked Tailscale nothing that signs it out: no request of any kind but reads.
    assert!(
        world
            .scene
            .fake()
            .requests()
            .iter()
            .all(|r| r.starts_with("GET ")),
        "{:?}",
        world.scene.fake().requests()
    );
    assert_eq!(world.scene.fake().logins(), 0);
}

// ---- Tailnet1 ----

#[tokio::test(flavor = "multi_thread")]
async fn machines_lists_every_kind_of_computer_and_not_this_one() {
    let scene = Scene::with(
        "machines",
        Daemon::Running(network()),
        vec![tailscale_account(AccountState::Ok)],
    )
    .await;
    let world = World::start(scene, true).await;
    let machines = world
        .machines_as(CallerRole::Terminal)
        .await
        .expect("machines");
    let by = |name: &str| machines.iter().find(|m| m.name == name).expect(name);
    assert_eq!(machines.len(), 4);
    assert!(machines.iter().all(|m| m.node.as_str() != "nSELF"));
    // Mine, shared and tagged.
    assert_eq!(by("pi").owner, MachineOwner::Mine);
    assert_eq!(by("friends-pc").owner, MachineOwner::Shared);
    assert_eq!(by("build-box").owner, MachineOwner::Tagged);
    // Online with SSH, and no last seen; offline with a last seen and no SSH.
    let pi = by("pi");
    assert!(pi.online && pi.ssh && pi.last_seen.is_none());
    assert_eq!(pi.ssh_host_keys, ["ssh-ed25519 AAAApi"]);
    assert_eq!(pi.node.as_str(), "nPI");
    assert_eq!(pi.dns, "pi.tail1234.ts.net");
    assert_eq!(pi.addresses[0].to_string(), "100.64.0.2");
    assert_eq!(pi.os, "linux");
    let old = by("old-laptop");
    assert!(!old.online && !old.ssh && old.ssh_host_keys.is_empty());
    assert_eq!(old.last_seen.map(|t| t.0), Some(1_790_000_000));
    // The shell and Settings read the same list.
    for role in [CallerRole::SheetHost, CallerRole::Settings] {
        assert_eq!(world.machines_as(role).await.expect("machines"), machines);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn machines_is_empty_with_no_account_and_with_one_that_is_not_working() {
    // No Tailscale account in porter: empty, though Tailscale is running.
    let scene = Scene::with("empty-no-account", Daemon::Running(network()), vec![]).await;
    let world = World::start(scene, true).await;
    assert_eq!(
        world.machines_as(CallerRole::Terminal).await.expect("list"),
        Vec::<Machine>::new()
    );

    // An account, and Tailscale signed out: empty, not an error.
    let scene = Scene::with(
        "empty-signed-out",
        Daemon::SignedOut {
            auth_url: LOGIN_PAGE.to_owned(),
        },
        vec![tailscale_account(AccountState::Ok)],
    )
    .await;
    let world = World::start(scene, true).await;
    assert!(
        world
            .machines_as(CallerRole::Terminal)
            .await
            .expect("list")
            .is_empty()
    );

    // An account that is off: empty, even though Tailscale answers a list.
    let scene = Scene::with(
        "empty-offline",
        Daemon::Running(network()),
        vec![tailscale_account(AccountState::Offline)],
    )
    .await;
    // Not followed, so the account stays as the registry has it.
    let world = World::start(scene, false).await;
    assert!(
        world
            .machines_as(CallerRole::Terminal)
            .await
            .expect("list")
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_fires_once_for_a_burst_and_again_for_the_next_change() {
    let scene = Scene::with(
        "burst",
        Daemon::Running(network()),
        vec![tailscale_account(AccountState::Ok)],
    )
    .await;
    let world = World::start(scene, true).await;
    let reader = world
        .client("org.example.Reader", CallerRole::Terminal)
        .await;
    let proxy = TailnetProxy::new(&reader).await.expect("proxy");
    let mut changes = proxy.receive_changed().await.expect("subscribe");
    read(&reader).await.expect("machines");
    // An app that has called accountd hears nothing of the computers.
    let app = world.client("org.example.Files", CallerRole::App).await;
    let mut app_changes = TailnetProxy::new(&app)
        .await
        .expect("proxy")
        .receive_changed()
        .await
        .expect("subscribe");
    let _ = ManagerProxy::new(&app).await.expect("proxy");
    let _ = read(&app).await;

    // Ten changes in a burst: one signal.
    for round in 0..10 {
        world
            .scene
            .fake()
            .edit(|net| net.peers[1].name = format!("laptop-{round}"));
    }
    told(&mut changes, "a Changed for the burst").await;
    assert!(
        quiet_for(&mut changes, Duration::from_millis(900)).await,
        "a second signal for the same burst"
    );
    // The list is what the burst left.
    let machines = read(&reader).await.expect("machines");
    assert!(machines.iter().any(|m| m.name == "laptop-9"));
    // A later change is a later signal.
    world.scene.fake().edit(|net| net.peers[0].online = false);
    told(&mut changes, "a Changed for the later change").await;
    // The app was never told: it has heard nothing by the time the terminal has.
    assert!(quiet_for(&mut app_changes, Duration::from_millis(300)).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn who_may_read_the_computers() {
    let scene = Scene::with(
        "who-may-read",
        Daemon::Running(network()),
        vec![tailscale_account(AccountState::Ok)],
    )
    .await;
    let world = World::start(scene, true).await;
    // Step 1, the refused: an app is refused, and so are the roles that are not a person's own
    // windows.
    for role in [
        CallerRole::App,
        CallerRole::Agent,
        CallerRole::Cua,
        CallerRole::AgentLauncher,
    ] {
        let denied = world.machines_as(role).await.expect_err("refused");
        assert_eq!(
            error_name(&denied),
            common::ACCESS_DENIED,
            "{role:?} may not read the computers"
        );
    }
    // Step 2, a sender accountd does not know.
    let stranger = world.bus.connect().await;
    let denied = read(&stranger).await.expect_err("refused");
    assert_eq!(
        error_name(&denied),
        common::ACCESS_DENIED,
        "step 2: a stranger"
    );
    // Step 3, the AI broker: inferd (`PorterDaemon`) gets the same list as the shell, to find
    // the computers that lend its models.
    let rows = world
        .machines_as(CallerRole::PorterDaemon)
        .await
        .expect("step 3: the broker reads the computers");
    assert_eq!(rows.len(), 4, "step 3: the broker's list");
    // Step 4, the terminal reads the computers and is refused everything else.
    let terminal = world.client("org.quire.Temor", CallerRole::Terminal).await;
    assert_eq!(
        read(&terminal).await.expect("step 4: machines").len(),
        4,
        "step 4: the terminal's list"
    );
    let spaces = SpacesProxy::new(&terminal).await.expect("proxy");
    let denied = spaces.list().await.expect_err("refused");
    assert_eq!(
        error_name(&denied),
        common::ACCESS_DENIED,
        "step 4: the terminal may read nothing else"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_renamed_computer_keeps_its_node_id() {
    let scene = Scene::with(
        "renamed",
        Daemon::Running(network()),
        vec![tailscale_account(AccountState::Ok)],
    )
    .await;
    let world = World::start(scene, true).await;
    let terminal = world.client("org.quire.Temor", CallerRole::Terminal).await;
    let before = read(&terminal).await.expect("machines");
    let old = before
        .iter()
        .find(|m| m.name == "old-laptop")
        .expect("old-laptop")
        .clone();
    // The stable id of Tailscale (what the fake writes as `ID` in status), not a name.
    assert_eq!(old.node.as_str(), "nOLD");

    // The person renames it in Tailscale; its address changes too.
    world.scene.fake().edit(|net| {
        let peer = &mut net.peers[1];
        peer.name = "studio".into();
        peer.addresses = vec!["100.64.0.30".into()];
    });
    let after = read(&terminal).await.expect("machines");
    assert_eq!(after.len(), before.len());
    let renamed = after
        .iter()
        .find(|m| m.node == old.node)
        .expect("the same node id");
    assert_eq!(renamed.name, "studio");
    assert_eq!(renamed.dns, "studio.tail1234.ts.net");
    assert_eq!(renamed.addresses[0].to_string(), "100.64.0.30");
    assert!(after.iter().all(|m| m.name != "old-laptop"));
}
