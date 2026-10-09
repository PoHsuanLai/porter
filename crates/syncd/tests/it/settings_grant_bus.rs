//! Settings lets syncd sync an account, end to end on one private bus: the real accountd's
//! settings module, a Graph account that holds no grant, syncd's storage supervisor and a fake
//! Graph drive. Switching "Keep this account's files on this computer" on makes the grant
//! `org.quire.Sync` is asked for and the mirror starts; off takes it back, the dataset goes and
//! the files stay. Nothing seeds a grant by hand.

use crate::common;

use accountd::{BusSheets, Options, TableCallers, serve_with};
use common::bus::PrivateBus;
use common::{Known, client};
use ds_settings::live::LiveClient;
use ds_settings::schema::KeyPath;
use porter_client::{Accounts, DbusTransport};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AuthKind, Claim, Credential, EndpointUrl,
    Family, LoginName, Offer, Provenance, Restriction, SecretKey, SecretPurpose, SecretText,
    ServiceEndpoint, Subject, Tls, UnixSeconds,
};
use porter_dbus::{Caller, CallerRole, SyncProxy};
use porter_fake::{FakeProvider, FixedClock, MemoryStore, RecordingAudit};
use porter_fake_servers::{FakeGraph, GraphHandle, Running};
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry, SyncClass, sync_allowed, sync_app};
use std::sync::Arc;
use std::time::Duration;
use syncd::datasets::photos::PhotosSwitch;
use syncd::datasets::pim::Wiring;
use syncd::datasets::storage::{
    ClientStorageGrants, FILES_APP, PHOTOS_APP, StorageConfig, StorageSupervisor,
};
use syncd::paths::{AccountDir, Paths};
use syncd::scheduler::{Network, Settings};
use syncd::service::{Access, Hub, serve};
use tokio::sync::watch;

const ACCOUNT: &str = "graph-acct";
const SEGMENT: &str = "graph_acct";
const BEARER: &str = "fake:graph-acct:graph";
const DATASET: &str = "graph_acct/storage_app_folder";
const FILES_ROW: &str = "accounts.graph-acct.sync.files";
const PHOTOS_ROW: &str = "accounts.graph-acct.sync.photos";

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
"#;

type Supervisor = StorageSupervisor<DbusTransport, ClientStorageGrants<DbusTransport>>;

struct Rig {
    bus: PrivateBus,
    known: Known,
    graph: Running<GraphHandle>,
    paths: Paths,
    account: AccountDir,
    service: Arc<
        AccountService<
            FakeProvider,
            MemorySecrets,
            BusSheets<TableCallers>,
            FixedClock,
            MemoryStore,
            RecordingAudit,
        >,
    >,
    settings: LiveClient,
    supervisor: Supervisor,
    _keep: Vec<zbus::Connection>,
    _network: watch::Sender<Network>,
}

fn quick() -> Settings {
    Settings::quick()
}

async fn rig(photos: PhotosSwitch) -> Rig {
    let graph = FakeGraph::start(BEARER).await.expect("graph");
    graph.set_limit(1_000_000);
    let provider = FakeProvider::from_file(PROVIDER);
    let spec = provider.spec();
    let account = Account {
        id: AccountId::parse(ACCOUNT).expect("id"),
        provider: spec.id.clone(),
        label: AccountLabel("ada@outlook.test".into()),
        state: AccountState::Ok,
        auth: AuthKind::OAuthPkce,
        capabilities: spec
            .capabilities
            .iter()
            .map(|row| Claim {
                subject: Subject::Account,
                offer: Offer::Present(row.capability.clone()),
                provenance: Provenance::Declared,
            })
            .collect(),
        restriction: Restriction::none(),
        endpoints: vec![ServiceEndpoint {
            family: Family::Graph,
            url: EndpointUrl::parse(graph.base_url()).expect("url"),
            tls: Tls::Plain,
            login: LoginName("ada@outlook.test".into()),
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
            },
        )
        .await
        .expect("secret");

    let bus = PrivateBus::start();
    let table = Arc::new(TableCallers::new());
    let accountd = bus.connect().await;
    let sheets = BusSheets::new(accountd.clone(), Arc::clone(&table));
    // No grant at all: the person has to switch the sync on.
    let service = Arc::new(
        AccountService::new(
            vec![provider],
            Registry {
                grants: vec![],
                accounts: vec![account],
                toggles: vec![],
            },
            secrets,
            sheets,
            FixedClock(porter_fake::NOW),
        )
        .with_store(MemoryStore::default())
        .with_audit(RecordingAudit::default()),
    );
    serve_with(
        &accountd,
        Arc::clone(&service),
        Arc::clone(&table),
        Options::default(),
    )
    .await
    .expect("accountd serves");

    // syncd reaches accountd as the native daemon the caller table names.
    let syncd_side = bus.connect().await;
    table.introduce_as(
        syncd_side.unique_name().expect("name").as_str(),
        Caller {
            app: sync_app(),
            role: CallerRole::App,
        },
    );
    let accounts = Arc::new(Accounts::over(DbusTransport::over(syncd_side.clone())));
    let settings_side = bus.connect().await;
    table.introduce_as(
        settings_side.unique_name().expect("name").as_str(),
        Caller {
            app: common::app("org.quire.Settings"),
            role: CallerRole::Settings,
        },
    );
    let settings = LiveClient::new(
        &settings_side,
        "org.quire.Accounts1",
        &accountd::settings_path(),
    )
    .await
    .expect("settings client");

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
        accounts: Arc::clone(&accounts),
        hub,
        paths: paths.clone(),
        settings: quick(),
        network,
        owners: Access::default(),
    };
    let supervisor = StorageSupervisor::new(
        wiring,
        ClientStorageGrants::new(accounts),
        StorageConfig {
            photos,
            ..StorageConfig::default()
        },
    );
    Rig {
        bus,
        known,
        graph,
        paths,
        account: AccountDir::parse(SEGMENT).expect("segment"),
        service,
        settings,
        supervisor,
        _keep: vec![accountd, syncd_side, settings_side, server],
        _network: network_keeps,
    }
}

fn row(path: &str) -> KeyPath {
    KeyPath(path.to_owned())
}

fn word(on: bool) -> toml::Value {
    toml::Value::String(if on { "on" } else { "off" }.to_owned())
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    deadline.fail(what);
}

async fn names(rig: &Rig, app: &str) -> Vec<String> {
    let connection = client(&rig.bus, &rig.known, app, CallerRole::App).await;
    let mut seen = SyncProxy::new(&connection)
        .await
        .expect("proxy")
        .datasets()
        .await
        .expect("datasets");
    seen.sort();
    seen
}

fn local(rig: &Rig, rel: &str) -> std::path::PathBuf {
    rig.paths.storage_dir(&rig.account).join(rel)
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_files_on_in_settings_starts_the_mirror_and_off_drops_it_but_keeps_the_files() {
    let mut rig = rig(PhotosSwitch::Off).await;
    let schema = rig.settings.describe().await.expect("schema");
    let paths: Vec<&str> = schema.key.iter().map(|k| k.path.0.as_str()).collect();
    assert!(
        paths.contains(&FILES_ROW) && paths.contains(&PHOTOS_ROW),
        "{paths:?}"
    );
    assert_eq!(
        rig.settings.get(&row(FILES_ROW)).await.expect("get"),
        word(false)
    );

    // Without the switch syncd finds no grant and mirrors nothing.
    rig.supervisor.tick().await;
    assert!(names(&rig, FILES_APP).await.is_empty());

    rig.settings
        .set(&row(FILES_ROW), &word(true))
        .await
        .expect("on");
    assert_eq!(
        rig.settings.get(&row(FILES_ROW)).await.expect("get"),
        word(true)
    );
    let grants = rig.service.registry().grants;
    assert!(sync_allowed(
        &grants,
        &AccountId::parse(ACCOUNT).expect("id"),
        SyncClass::Files
    ));
    assert_eq!(grants.len(), 1, "only the files grant");

    rig.supervisor.tick().await;
    assert_eq!(names(&rig, FILES_APP).await, [DATASET]);
    rig.graph.put_file("note.txt", b"from the drive");
    eventually("the file arrives", || local(&rig, "note.txt").exists()).await;

    rig.settings
        .set(&row(FILES_ROW), &word(false))
        .await
        .expect("off");
    assert_eq!(
        rig.settings.get(&row(FILES_ROW)).await.expect("get"),
        word(false)
    );
    assert!(rig.service.registry().grants.is_empty());
    rig.supervisor.tick().await;
    assert!(
        names(&rig, FILES_APP).await.is_empty(),
        "the dataset dropped"
    );
    assert!(rig.supervisor.datasets().is_empty());
    assert!(
        local(&rig, "note.txt").exists(),
        "the files are the person's"
    );

    // On again picks the folder up where it was.
    rig.settings
        .set(&row(FILES_ROW), &word(true))
        .await
        .expect("on again");
    rig.supervisor.tick().await;
    assert_eq!(names(&rig, FILES_APP).await, [DATASET]);
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_photos_on_starts_the_photos_datasets_beside_the_files_ones_when_photos_run() {
    let mut rig = rig(PhotosSwitch::On).await;
    rig.settings
        .set(&row(PHOTOS_ROW), &word(true))
        .await
        .expect("photos on");
    rig.supervisor.tick().await;
    assert_eq!(
        names(&rig, PHOTOS_APP).await,
        ["graph_acct/photos_metadata", "graph_acct/photos_originals"]
    );
    assert!(
        names(&rig, FILES_APP).await.is_empty(),
        "photos alone, no app folder mirror"
    );

    rig.settings
        .set(&row(PHOTOS_ROW), &word(false))
        .await
        .expect("photos off");
    rig.supervisor.tick().await;
    assert!(names(&rig, PHOTOS_APP).await.is_empty());
}
