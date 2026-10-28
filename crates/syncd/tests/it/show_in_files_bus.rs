//! "Show in Files", end to end on one private bus: syncd's own settings module over its own
//! hosted account service, a Microsoft-shaped account signed in before it was given the whole
//! drive (its record claims only the app folder), syncd's storage supervisor and a fake Graph
//! drive. Turning the row on asks the person for the whole drive in a sign-in of its own, then
//! records syncd's grant of it, and the supervisor lists the drive; refused, the row reads off with
//! a plain line under it; off takes the grant back, keeps the listing and the access given, so on
//! again asks nothing. A listed drive's files download on demand into its cache, the cache limit
//! row sets what eviction keeps to, "Keep Everything Downloaded" (offered only while the drive is
//! shown) keeps the whole drive and what is added to it, also after a restart, and a computer
//! with no way to show the folder says so.

use crate::common;

use common::bus::PrivateBus;
use common::hosted::{Hosted, Link, NoDirectory, Service, host_with};
use common::{Known, client};
use ds_settings::live::{AnyOwner, LiveClient};
use ds_settings::schema::KeyPath;
use porter_callers::CallerRole;
use porter_capability::KindToggle;
use porter_capability::{CapabilityKind, StorageScope};
use porter_capability::{Claim, Offer, Provenance, Subject, Toggle};
use porter_core::AccountId;
use porter_core::{
    Account, AccountLabel, AccountState, AuthKind, Credential, EndpointUrl, Family, LoginName,
    Restriction, SecretKey, SecretPurpose, SecretText, ServiceEndpoint, Tls,
};
use porter_drive::{CacheLimit, FreeSpace, ItemState, Reason};
use porter_drive_fuse::{Mounter, mountd};
use porter_fake::{FakeProvider, ScriptedSheets};
use porter_fake_servers::{FakeGraph, GraphHandle, Running};
use porter_families::set::ProviderNames;
use porter_ids::UnixSeconds;
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service_core::registry::Registry;
use porter_sheet::SheetInput;
use porter_sync::ItemPath;
use std::sync::Arc;
use std::time::Duration;
use syncd::datasets::drive::LiveDrive;
use syncd::datasets::drive::{DriveJournal, journal_path};
use syncd::datasets::pim::Wiring;
use syncd::datasets::storage::{ClientStorageGrants, DriveMount, StorageConfig, StorageSupervisor};
use syncd::keep::{SyncClass, claims_whole_drive, drive_allowed, sync_allowed};
use syncd::paths::{AccountDir, Paths};
use syncd::scheduler::{Network, Settings};
use syncd::service::{Access, Hub, serve};
use syncd::settings::{CACHE_LIMIT, FILES_NOT_ON, SETTINGS_PATH, SyncSettings, serve_settings};
use syncd::words;
use tokio::sync::watch;

const ACCOUNT: &str = "graph-acct";
const SEGMENT: &str = "graph_acct";
const BEARER: &str = "fake:graph-acct:graph";
const FILES_ROW: &str = "accounts.graph-acct.storage.files";
const KEEP_ROW: &str = "accounts.graph-acct.storage.keep";

/// A Microsoft-shaped provider: the app folder asked at sign-in, the whole drive on use.
const PROVIDER: &str = r#"
id = "fake-graph"
label = "Fake Microsoft"
mark = "generic"

[auth]
kind = "oauth_pkce"
issuer = "microsoft"

[discovery]
kind = "autoconfig"

[[capability]]
family = "graph"
kind = "storage"
v = { access = "read_write", delta = "poll", quota = "reported", scope = "app_folder", hashes = "quick_xor", ranges = "present", chunked_upload = "present" }

[[capability]]
family = "graph"
kind = "storage"
ask = "on_use"
v = { access = "read_write", delta = "poll", quota = "reported", scope = "full", hashes = "quick_xor", ranges = "present", chunked_upload = "present" }
"#;

struct Rig {
    graph: Running<GraphHandle>,
    paths: Paths,
    account: AccountDir,
    hosted: Hosted,
    settings: LiveClient,
    supervisor: StorageSupervisor<Link, ClientStorageGrants<Link>>,
    _bus: PrivateBus,
    _keep: Vec<zbus::Connection>,
    _network: watch::Sender<Network>,
}

/// The rig, its sheet host answering as `sheets` says; no drive's folder is shown.
async fn rig(sheets: ScriptedSheets) -> Rig {
    rig_showing(sheets, |_| DriveMount::Off).await
}

/// The rig, each drive's folder shown as `mount` says (given the scratch directory).
async fn rig_showing(
    sheets: ScriptedSheets,
    mount: impl FnOnce(&std::path::Path) -> DriveMount,
) -> Rig {
    build(sheets, mount, FreeSpace::Measured).await
}

/// The rig, each drive holding this much free space to keep it downloaded whole.
async fn rig_with_room(sheets: ScriptedSheets, room: FreeSpace) -> Rig {
    build(sheets, |_| DriveMount::Off, room).await
}

async fn build(
    sheets: ScriptedSheets,
    mount: impl FnOnce(&std::path::Path) -> DriveMount,
    room: FreeSpace,
) -> Rig {
    let graph = FakeGraph::start(BEARER).await.expect("graph");
    let provider = FakeProvider::from_file(PROVIDER);
    let spec = provider.spec().clone();
    let login = LoginName("ada@outlook.test".into());
    // Signed in before the whole drive was asked: the record claims what sign-in asked only.
    let account = Account {
        id: AccountId::parse(ACCOUNT).expect("id"),
        provider: spec.id.clone(),
        label: AccountLabel("ada@outlook.test".into()),
        state: AccountState::Ok,
        auth: AuthKind::OAuthPkce,
        capabilities: spec
            .capabilities
            .iter()
            .filter(|row| row.asked_at_sign_in())
            .map(|row| Claim {
                subject: Subject::Account,
                offer: Offer::Present(row.capability.clone()),
                provenance: Provenance::Probed,
            })
            .collect(),
        restriction: Restriction::none(),
        endpoints: vec![ServiceEndpoint {
            family: Family::Graph,
            url: EndpointUrl::parse(graph.base_url()).expect("url"),
            tls: Tls::Plain,
            login,
        }],
    };
    let secrets = MemorySecrets::default();
    secrets
        .put(
            &SecretKey {
                account: account.id.clone(),
                purpose: SecretPurpose::OAuthRefresh,
            },
            &Credential::OAuth {
                access: SecretText::new("SECRET-ACCESS-TOKEN"),
                refresh: SecretText::new("SECRET-REFRESH-TOKEN"),
                expires_at: UnixSeconds(1),
                granted: Default::default(),
            },
        )
        .await
        .expect("secret");

    let bus = PrivateBus::start();
    let hosted = host_with(
        provider,
        Registry {
            grants: vec![],
            accounts: vec![account],
            toggles: vec![],
        },
        secrets,
        sheets,
    );
    let known = Known::default();
    let hub = Hub::default();
    let server = bus.connect().await;
    serve(&server, hub.clone(), known.clone())
        .await
        .expect("sync1");
    let home = bus.scratch().join("syncd-home");
    let paths = Paths::resolve(|name| match name {
        "HOME" => Some(home.display().to_string()),
        _ => None,
    })
    .expect("paths");

    let (network_keeps, network) = watch::channel(Network::Unmetered);
    let wiring = Wiring {
        accounts: Arc::clone(&hosted.accounts),
        hub: hub.clone(),
        paths: paths.clone(),
        settings: Settings::quick(),
        network,
        owners: Access::default(),
        clock: porter_sync::Clock::new(porter_ids::clock::SystemClock),
        nonce: "t1".to_owned(),
        millis: Arc::new(syncd::datasets::photos::ManualMillis::at(1_700_000_000_000)),
    };
    let supervisor = StorageSupervisor::new(
        wiring,
        ClientStorageGrants::new(Arc::clone(&hosted.accounts)),
        StorageConfig {
            drive_every: Duration::from_millis(100),
            mount: mount(bus.scratch()),
            free_space: room,
            ..StorageConfig::default()
        },
    );
    let own = common::hosted::keep(&hosted, NoDirectory, paths.clone(), hub.clone())
        .with_drives(supervisor.drive_set());
    let module = SyncSettings::new(
        Arc::new(own),
        Arc::new(known.clone()),
        ProviderNames::from_specs(std::slice::from_ref(&spec)),
    )
    .with_cache_limit(supervisor.cache_knob(), paths.settings.clone());
    serve_settings(&server, module).await.expect("settings");
    let name = server.unique_name().expect("name").to_string();
    let settings_side = client(&bus, &known, "org.quire.Settings", CallerRole::Settings).await;
    let settings =
        LiveClient::new(&settings_side, &name, SETTINGS_PATH, AnyOwner).expect("settings client");

    Rig {
        graph,
        paths,
        account: AccountDir::parse(SEGMENT).expect("segment"),
        hosted,
        settings,
        supervisor,
        _bus: bus,
        _keep: vec![server, settings_side],
        _network: network_keeps,
    }
}

/// Sheets whose one sign-in the person goes through: they review the whole drive and allow it.
fn agreeing() -> ScriptedSheets {
    ScriptedSheets::default().conversing([vec![SheetInput::Confirm(vec![KindToggle {
        kind: CapabilityKind::Storage,
        toggle: Toggle::On,
    }])]])
}

/// Sheets whose one sign-in the person closes.
fn declining() -> ScriptedSheets {
    ScriptedSheets::default().conversing([vec![SheetInput::Dismiss]])
}

fn row() -> KeyPath {
    KeyPath(FILES_ROW.to_owned())
}

fn word(on: bool) -> toml::Value {
    toml::Value::String(if on { "on" } else { "off" }.to_owned())
}

fn id() -> AccountId {
    AccountId::parse(ACCOUNT).expect("id")
}

fn service(rig: &Rig) -> &Service {
    &rig.hosted.service
}

async fn eventually(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let deadline = test_kit::Deadline::generous();
    while !deadline.passed() {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    deadline.fail(what);
}

fn listed(rig: &Rig, at: &str) -> bool {
    let Ok(journal) = DriveJournal::open(&journal_path(&rig.paths, &rig.account)) else {
        return false;
    };
    journal
        .entry_at(&ItemPath(at.into()))
        .is_ok_and(|entry| entry.is_some())
}

async fn note(rig: &Rig) -> Option<String> {
    let schema = rig.settings.describe().await.expect("schema");
    let key = schema
        .key
        .into_iter()
        .find(|key| key.path.0 == FILES_ROW)
        .expect("the row is offered");
    key.note.map(|note| note.0)
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_show_in_files_on_asks_for_the_whole_drive_then_the_drive_is_listed() {
    let mut rig = rig(agreeing()).await;
    rig.graph.put_root_file("Docs/Plan.txt", b"plan");
    assert_eq!(rig.settings.get(&row()).await.expect("get"), word(false));
    assert!(!claims_whole_drive(&service(&rig).registry().accounts[0]));

    rig.settings.set(&row(), &word(true)).await.expect("on");
    eventually("the row reads on", async || {
        rig.settings.get(&row()).await.ok() == Some(word(true))
    })
    .await;
    let registry = service(&rig).registry();
    assert!(
        claims_whole_drive(&registry.accounts[0]),
        "the access was given"
    );
    assert!(drive_allowed(&registry.grants, &id()));
    assert!(
        !sync_allowed(&registry.grants, &id(), SyncClass::Files),
        "the app folder mirror is a switch of its own"
    );
    let reaches: Vec<StorageScope> = registry.grants.iter().map(|g| g.key.reach).collect();
    assert_eq!(reaches, [StorageScope::Full]);
    assert_eq!(note(&rig).await, None);

    rig.supervisor.tick().await;
    assert_eq!(rig.supervisor.drives(), [rig.account.clone()]);
    assert!(
        rig.supervisor.datasets().is_empty(),
        "a listing is no dataset"
    );
    eventually("the drive is listed", async || {
        listed(&rig, "Docs/Plan.txt")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn show_in_files_the_person_does_not_agree_to_reads_off_with_a_plain_line() {
    let mut rig = rig(declining()).await;
    rig.settings.set(&row(), &word(true)).await.expect("asked");
    eventually("the row says it did not turn on", async || {
        note(&rig).await.as_deref() == Some(FILES_NOT_ON)
    })
    .await;
    assert_eq!(rig.settings.get(&row()).await.expect("get"), word(false));
    let registry = service(&rig).registry();
    assert!(registry.grants.is_empty(), "nothing was granted");
    assert!(!claims_whole_drive(&registry.accounts[0]));
    rig.supervisor.tick().await;
    assert!(rig.supervisor.drives().is_empty());
    for word in ["scope", "token", "consent", "WebDAV", "FUSE", "mount"] {
        assert!(!FILES_NOT_ON.contains(word), "{word}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_show_in_files_off_stops_the_listing_and_keeps_it_so_on_again_asks_nothing() {
    let mut rig = rig(agreeing()).await;
    rig.graph.put_root_file("Docs/Plan.txt", b"plan");
    rig.settings.set(&row(), &word(true)).await.expect("on");
    eventually("the row reads on", async || {
        rig.settings.get(&row()).await.ok() == Some(word(true))
    })
    .await;
    rig.supervisor.tick().await;
    eventually("the drive is listed", async || {
        listed(&rig, "Docs/Plan.txt")
    })
    .await;

    rig.settings.set(&row(), &word(false)).await.expect("off");
    assert_eq!(rig.settings.get(&row()).await.expect("get"), word(false));
    assert!(service(&rig).registry().grants.is_empty());
    rig.supervisor.tick().await;
    assert!(rig.supervisor.drives().is_empty(), "the listing stopped");
    assert!(listed(&rig, "Docs/Plan.txt"), "the listing's journal stays");
    assert!(
        claims_whole_drive(&service(&rig).registry().accounts[0]),
        "the access given stays"
    );

    // The sheet host has no second sign-in: on again must ask nothing of the person.
    rig.settings
        .set(&row(), &word(true))
        .await
        .expect("on again");
    eventually("the row reads on again", async || {
        rig.settings.get(&row()).await.ok() == Some(word(true))
    })
    .await;
    rig.supervisor.tick().await;
    assert_eq!(rig.supervisor.drives(), [rig.account.clone()]);
}

/// Turns the row on, lists the drive, and answers it as Files serves it.
async fn shown(rig: &mut Rig) -> Arc<LiveDrive> {
    rig.settings.set(&row(), &word(true)).await.expect("on");
    eventually("the row reads on", async || {
        rig.settings.get(&row()).await.ok() == Some(word(true))
    })
    .await;
    rig.supervisor.tick().await;
    eventually("the drive is listed", async || listed(rig, "Docs/Plan.txt")).await;
    let drives = rig.supervisor.drive_set();
    let drive = drives.get(rig.account.as_str()).expect("served");
    eventually("its items are named", async || {
        drive
            .drive
            .resolve("Docs/Plan.txt".as_ref())
            .is_ok_and(|n| n.is_some())
    })
    .await;
    drive
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listed_drive_downloads_a_file_into_its_cache_when_asked() {
    let mut rig = rig(agreeing()).await;
    rig.graph.put_root_file("Docs/Plan.txt", b"plan");
    let drive = shown(&mut rig).await;
    assert_eq!(drive.provider, "OneDrive");
    assert_eq!(drive.path, rig.paths.cloud.join("OneDrive"));
    let node = drive
        .drive
        .resolve("Docs/Plan.txt".as_ref())
        .expect("resolve")
        .expect("there");
    let status = drive.drive.status(&node.id).expect("status");
    assert_eq!(status.state, ItemState::Cloud);

    drive.drive.download(&node.id).expect("download");
    eventually("it is downloaded", async || {
        drive
            .drive
            .status(&node.id)
            .is_some_and(|s| s.state == ItemState::Downloaded)
    })
    .await;
    assert_eq!(drive.drive.local_bytes(), 4);
    assert_eq!(
        drive.drive.read(&node.id, 0, 64).await.expect("read"),
        b"plan"
    );
    let cache = rig.paths.drive_cache_dir(&rig.account);
    assert_eq!(drive.drive.cache_dir(), cache.as_path());
    assert!(cache.starts_with(rig._bus.scratch()), "a scratch cache");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cache_limit_row_sets_what_eviction_keeps_to() {
    let mut rig = rig(agreeing()).await;
    rig.graph.put_root_file("Docs/Plan.txt", b"plan");
    let drive = shown(&mut rig).await;
    let limit = KeyPath(CACHE_LIMIT.to_owned());
    let text = |t: &str| toml::Value::String(t.to_owned());
    assert_eq!(rig.settings.get(&limit).await.expect("get"), text("0"));
    assert_eq!(drive.drive.limit(), CacheLimit::Automatic);

    let node = drive
        .drive
        .resolve("Docs/Plan.txt".as_ref())
        .expect("resolve")
        .expect("there");
    drive.drive.download(&node.id).expect("download");
    eventually("it is downloaded", async || {
        drive
            .drive
            .status(&node.id)
            .is_some_and(|s| s.state == ItemState::Downloaded)
    })
    .await;

    // Smaller than the file: eviction removes it.
    rig.settings.set(&limit, &text("1")).await.expect("set");
    assert_eq!(rig.settings.get(&limit).await.expect("get"), text("1"));
    eventually("the drive follows the row", async || {
        drive.drive.limit() == CacheLimit::Bytes(1)
    })
    .await;
    eventually("the download is removed", async || {
        drive
            .drive
            .status(&node.id)
            .is_some_and(|s| s.state == ItemState::Cloud)
    })
    .await;
    assert_eq!(drive.drive.local_bytes(), 0);
    let saved = std::fs::read_to_string(&rig.paths.settings).expect("saved");
    assert!(saved.contains("cache_limit = \"1\""), "{saved}");

    // Kept, a file stays whatever the limit.
    drive.drive.keep(&node.id, true).expect("keep");
    eventually("kept is here", async || {
        drive
            .drive
            .status(&node.id)
            .is_some_and(|s| s.state == ItemState::Downloaded)
    })
    .await;
    rig.settings
        .set(&limit, &text("0"))
        .await
        .expect("automatic");
    eventually("automatic again", async || {
        drive.drive.limit() == CacheLimit::Automatic
    })
    .await;
    assert!(
        rig.settings.set(&limit, &text("lots")).await.is_err(),
        "bytes or 0 only"
    );
}

async fn offered(rig: &Rig, path: &str) -> bool {
    let schema = rig.settings.describe().await.expect("schema");
    schema.key.iter().any(|key| key.path.0 == path)
}

/// Where the item at `at` of `drive` stands, once it is named.
fn state_at(drive: &LiveDrive, at: &str) -> Option<ItemState> {
    let node = drive.drive.resolve(at.as_ref()).ok()??;
    drive.drive.status(&node.id).map(|status| status.state)
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_everything_downloaded_is_offered_while_shown_and_outlasts_a_restart() {
    let mut rig = rig(agreeing()).await;
    rig.graph.put_root_file("Docs/Plan.txt", b"plan");
    rig.graph.put_root_file("Notes.txt", b"notes");
    let keep_row = KeyPath(KEEP_ROW.to_owned());
    assert!(
        !offered(&rig, KEEP_ROW).await,
        "not while the drive isn't shown"
    );
    assert!(rig.settings.set(&keep_row, &word(true)).await.is_err());

    rig.settings.set(&row(), &word(true)).await.expect("on");
    eventually("the row reads on", async || {
        rig.settings.get(&row()).await.ok() == Some(word(true))
    })
    .await;
    assert!(offered(&rig, KEEP_ROW).await, "offered once shown");
    assert_eq!(rig.settings.get(&keep_row).await.expect("get"), word(false));
    // Set before the drive starts, as it is found after a restart: kept in syncd.toml.
    rig.settings
        .set(&keep_row, &word(true))
        .await
        .expect("keep all");
    assert_eq!(rig.settings.get(&keep_row).await.expect("get"), word(true));
    let saved = std::fs::read_to_string(&rig.paths.settings).expect("saved");
    assert!(saved.contains("keep_all = [\"graph_acct\"]"), "{saved}");

    rig.supervisor.tick().await;
    let drives = rig.supervisor.drive_set();
    let drive = drives.get(rig.account.as_str()).expect("served");
    assert!(
        drive.drive.keeps_all(),
        "the drive started keeping everything"
    );
    eventually("the whole drive is downloaded", async || {
        state_at(&drive, "Docs/Plan.txt") == Some(ItemState::Downloaded)
            && state_at(&drive, "Notes.txt") == Some(ItemState::Downloaded)
    })
    .await;
    // What is added to the drive is downloaded too, and nothing is evicted past the limit.
    rig.graph.put_root_file("Docs/Later.txt", b"later");
    eventually("the new file is downloaded", async || {
        state_at(&drive, "Docs/Later.txt") == Some(ItemState::Downloaded)
    })
    .await;
    let limit = KeyPath(CACHE_LIMIT.to_owned());
    rig.settings
        .set(&limit, &toml::Value::String("1".to_owned()))
        .await
        .expect("limit");
    eventually("the drive follows the limit", async || {
        drive.drive.limit() == CacheLimit::Bytes(1)
    })
    .await;
    for at in ["Docs/Plan.txt", "Notes.txt", "Docs/Later.txt"] {
        assert_eq!(state_at(&drive, at), Some(ItemState::Downloaded), "{at}");
    }
    let schema = rig.settings.describe().await.expect("schema");
    let keep = schema
        .key
        .iter()
        .find(|k| k.path.0 == KEEP_ROW)
        .expect("row");
    assert_eq!(keep.label.0, "Keep Everything Downloaded");
    assert!(keep.note.is_none(), "it fits");

    // Off: nothing is removed then, and what is past the limit may be evicted again.
    rig.settings
        .set(&keep_row, &word(false))
        .await
        .expect("keep all off");
    assert!(!drive.drive.keeps_all());
    rig.settings
        .set(&limit, &toml::Value::String("2".to_owned()))
        .await
        .expect("limit");
    eventually("evictable again", async || {
        state_at(&drive, "Notes.txt") == Some(ItemState::Cloud)
    })
    .await;

    // Show in Files off takes the row away.
    rig.settings.set(&row(), &word(false)).await.expect("off");
    assert!(!offered(&rig, KEEP_ROW).await);
}

/// The next `Changed` of `row`.
async fn next_change_of(changes: &mut ds_settings::live::Changes, row: &KeyPath) {
    use zbus::export::futures_core::Stream;
    loop {
        let change = tokio::time::timeout(
            test_kit::GENEROUS,
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut *changes).poll_next(cx)),
        )
        .await
        .expect("a Changed in time")
        .expect("open stream")
        .expect("a change");
        if change.key == *row {
            return;
        }
    }
}

async fn keep_note(rig: &Rig) -> Option<String> {
    let schema = rig.settings.describe().await.expect("schema");
    let key = schema
        .key
        .into_iter()
        .find(|key| key.path.0 == KEEP_ROW)
        .expect("the row is offered");
    key.note.map(|note| note.0)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_keep_row_is_told_of_when_the_drive_stops_fitting_and_again_when_it_fits() {
    let mut rig = rig_with_room(agreeing(), FreeSpace::Bytes(10)).await;
    rig.graph.put_root_file("Docs/Plan.txt", b"plan");
    let keep_row = KeyPath(KEEP_ROW.to_owned());
    rig.settings.set(&row(), &word(true)).await.expect("on");
    eventually("the row reads on", async || {
        rig.settings.get(&row()).await.ok() == Some(word(true))
    })
    .await;
    rig.settings
        .set(&keep_row, &word(true))
        .await
        .expect("keep all");
    rig.supervisor.tick().await;
    let drives = rig.supervisor.drive_set();
    let drive = drives.get(rig.account.as_str()).expect("served");
    eventually("it fits and is downloaded", async || {
        state_at(&drive, "Docs/Plan.txt") == Some(ItemState::Downloaded)
    })
    .await;
    assert_eq!(keep_note(&rig).await, None);

    // Twenty bytes more than the ten free: the next listing pass finds it short, and the row is
    // told of so the pane draws the line.
    let mut changes = rig.settings.changes().await.expect("changes");
    rig.graph.put_root_file("Big.bin", &[7; 20]);
    next_change_of(&mut changes, &keep_row).await;
    assert!(drive.drive.short_of_room());
    assert_eq!(
        keep_note(&rig).await.as_deref(),
        Some(words::reason(Reason::NoRoomForAll))
    );
    assert_eq!(state_at(&drive, "Big.bin"), Some(ItemState::Cloud));

    // Gone from the drive: it fits again, and the row is told of again.
    rig.graph.delete_root_item("Big.bin");
    next_change_of(&mut changes, &keep_row).await;
    assert!(!drive.drive.short_of_room());
    assert_eq!(keep_note(&rig).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_computer_that_cannot_show_the_folder_says_so_and_the_drive_still_lists() {
    let mut rig = rig_showing(agreeing(), |scratch| {
        // No porter-mountd listens at its socket, so syncd mounts by itself, as when run by
        // hand, and this computer has no FUSE device.
        DriveMount::Fuse(Mounter {
            device: scratch.join("no-fuse-device"),
            helper: "fusermount3".to_owned(),
            mountd: Some(mountd::wire::socket_in(&scratch.join("run"))),
        })
    })
    .await;
    rig.graph.put_root_file("Docs/Plan.txt", b"plan");
    let drive = shown(&mut rig).await;
    // The folder is mounted off the runtime's thread: its answer comes a moment later.
    eventually("the drive says it is not shown", async || {
        drive.state().0 == porter_dbus::sync::DriveState::Error
    })
    .await;
    let (_, said) = drive.state();
    assert_eq!(said.as_deref(), Some(words::NO_FILES_HERE));
    for word in ["mount", "FUSE", "fuse"] {
        assert!(!words::NO_FILES_HERE.contains(word), "{word}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn porter_mountd_is_asked_when_it_listens_and_its_words_are_shown() {
    let mut rig = rig_showing(agreeing(), |scratch| {
        // The drive's folder already holds a file of the person's: the helper refuses before it
        // would run fusermount3.
        let folder = scratch.join("syncd-home/Cloud/OneDrive");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("mine.txt"), b"mine").expect("file");
        let socket = mountd::wire::socket_in(&scratch.join("run"));
        let listener = mountd::server::bind_at(&socket).expect("bind");
        let helper = mountd::server::Helper::new(
            scratch.join("syncd-home/Cloud"),
            mountd::server::Fusermount::new(scratch.join("no-fusermount3").display().to_string()),
        );
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("asked");
            let _ = helper.answer(&stream);
        });
        DriveMount::Fuse(Mounter {
            device: scratch.join("no-fuse-device"),
            mountd: Some(socket),
            ..Mounter::new("fusermount3")
        })
    })
    .await;
    rig.graph.put_root_file("Docs/Plan.txt", b"plan");
    let drive = shown(&mut rig).await;
    // The folder is mounted off the runtime's thread: its answer comes a moment later.
    eventually("the drive says it is not shown", async || {
        drive.state().0 == porter_dbus::sync::DriveState::Error
    })
    .await;
    let (_, said) = drive.state();
    assert_eq!(
        said.as_deref(),
        Some(syncd::words::refusal(mountd::wire::Code::FolderInUse))
    );
}
