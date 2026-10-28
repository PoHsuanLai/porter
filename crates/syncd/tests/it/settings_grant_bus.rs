//! Settings lets syncd sync an account, end to end on one private bus: syncd's own settings module
//! (`/org/quire/Sync1/settings`) over its own hosted account service, a Graph account that holds
//! no grant, syncd's storage supervisor and a fake Graph drive. Switching "Keep this account's
//! files on this computer" on makes the grant `org.quire.Sync` is asked for and the mirror starts;
//! off takes it back, the dataset goes and the files stay. The Photos row does the same for the
//! Photos datasets, and is all that runs them. Nothing seeds a grant by hand. Only the Settings
//! role may use the module.

use crate::common;

use common::bus::PrivateBus;
use common::hosted::{Hosted, Link, NoDirectory, Service, host, keep};
use common::{Known, client};
use ds_settings::live::{AnyOwner, LiveClient};
use ds_settings::schema::KeyPath;
use porter_callers::CallerRole;
use porter_capability::{Claim, Offer, Provenance, Subject};
use porter_core::AccountId;
use porter_core::{
    Account, AccountLabel, AccountState, AuthKind, Credential, EndpointUrl, Family, LoginName,
    Restriction, SecretKey, SecretPurpose, SecretText, ServiceEndpoint, Tls,
};
use porter_dbus::sync::SyncProxy;
use porter_fake::FakeProvider;
use porter_fake_servers::{FakeGraph, GraphHandle, Running};
use porter_families::set::ProviderNames;
use porter_ids::UnixSeconds;
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service_core::registry::Registry;
use std::sync::Arc;
use std::time::Duration;
use syncd::datasets::pim::Wiring;
use syncd::datasets::storage::{
    ClientStorageGrants, FILES_APP, PHOTOS_APP, StorageConfig, StorageSupervisor,
};
use syncd::keep::{SyncClass, sync_allowed};
use syncd::paths::{AccountDir, Paths};
use syncd::scheduler::{Network, Settings};
use syncd::service::{Access, Hub, serve};
use syncd::settings::{SETTINGS_PATH, SyncSettings, serve_settings};
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

struct Rig {
    bus: PrivateBus,
    known: Known,
    graph: Running<GraphHandle>,
    paths: Paths,
    account: AccountDir,
    hosted: Hosted,
    settings: LiveClient,
    /// A connection that is not the Settings role.
    stranger: LiveClient,
    supervisor: StorageSupervisor<Link, ClientStorageGrants<Link>>,
    _keep: Vec<zbus::Connection>,
    _network: watch::Sender<Network>,
}

async fn rig() -> Rig {
    let graph = FakeGraph::start(BEARER).await.expect("graph");
    graph.set_limit(1_000_000);
    let provider = FakeProvider::from_file(PROVIDER);
    let spec = provider.spec().clone();
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
                granted: Default::default(),
            },
        )
        .await
        .expect("secret");

    let bus = PrivateBus::start();
    // No grant at all: the person has to switch the sync on.
    let hosted = host(
        provider,
        Registry {
            grants: vec![],
            accounts: vec![account],
            toggles: vec![],
        },
        secrets,
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

    // The settings module, as the daemon serves it, with the callers the test names.
    let own = keep(&hosted, NoDirectory, paths.clone(), hub.clone());
    let module = SyncSettings::new(
        Arc::new(own),
        Arc::new(known.clone()),
        ProviderNames::from_specs(std::slice::from_ref(&spec)),
    );
    serve_settings(&server, module).await.expect("settings");
    let name = server.unique_name().expect("name").to_string();
    let settings_side = client(&bus, &known, "org.quire.Settings", CallerRole::Settings).await;
    let settings =
        LiveClient::new(&settings_side, &name, SETTINGS_PATH, AnyOwner).expect("settings client");
    let stranger_side = client(&bus, &known, "org.example.Other", CallerRole::App).await;
    let stranger =
        LiveClient::new(&stranger_side, &name, SETTINGS_PATH, AnyOwner).expect("stranger client");

    let (network_keeps, network) = watch::channel(Network::Unmetered);
    let wiring = Wiring {
        accounts: Arc::clone(&hosted.accounts),
        hub,
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
        StorageConfig::default(),
    );
    Rig {
        bus,
        known,
        graph,
        paths,
        account: AccountDir::parse(SEGMENT).expect("segment"),
        hosted,
        settings,
        stranger,
        supervisor,
        _keep: vec![server, settings_side, stranger_side],
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
    let deadline = test_kit::Deadline::generous();
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

fn grants(rig: &Rig) -> Vec<porter_core::consent::Grant> {
    let service: &Service = &rig.hosted.service;
    service.registry().grants
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_files_on_in_settings_starts_the_mirror_and_off_drops_it_but_keeps_the_files() {
    let mut rig = rig().await;
    let schema = rig.settings.describe().await.expect("schema");
    let paths: Vec<&str> = schema.key.iter().map(|k| k.path.0.as_str()).collect();
    assert!(
        paths.contains(&FILES_ROW) && paths.contains(&PHOTOS_ROW),
        "{paths:?}"
    );
    assert!(paths.contains(&"accounts.sync.add"), "{paths:?}");
    assert!(
        !paths.contains(&"accounts.graph-acct.storage.files"),
        "a provider that never asks the whole drive offers no Show in Files: {paths:?}"
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
    let held = grants(&rig);
    assert!(sync_allowed(
        &held,
        &AccountId::parse(ACCOUNT).expect("id"),
        SyncClass::Files
    ));
    assert_eq!(held.len(), 1, "only the files grant");

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
    assert!(grants(&rig).is_empty());
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
async fn the_photos_row_alone_turns_the_photos_datasets_on_and_off_and_keeps_the_library() {
    let mut rig = rig().await;
    assert_eq!(
        rig.settings.get(&row(PHOTOS_ROW)).await.expect("get"),
        word(false)
    );
    rig.supervisor.tick().await;
    assert!(
        names(&rig, PHOTOS_APP).await.is_empty(),
        "off: nothing runs"
    );

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
    assert!(rig.supervisor.photos_library(&rig.account).is_some());

    // Off stops them as a Files grant that goes does, and keeps what they kept.
    rig.settings
        .set(&row(PHOTOS_ROW), &word(false))
        .await
        .expect("photos off");
    rig.supervisor.tick().await;
    assert!(names(&rig, PHOTOS_APP).await.is_empty());
    assert!(rig.supervisor.photos_library(&rig.account).is_none());
    assert!(
        rig.paths.photos_dir(&rig.account).is_dir(),
        "the library stays"
    );

    rig.settings
        .set(&row(PHOTOS_ROW), &word(true))
        .await
        .expect("photos on again");
    rig.supervisor.tick().await;
    assert_eq!(names(&rig, PHOTOS_APP).await.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_settings_role_may_read_or_change_the_rows() {
    let rig = rig().await;
    assert!(rig.stranger.describe().await.is_err());
    assert!(rig.stranger.get(&row(FILES_ROW)).await.is_err());
    assert!(
        rig.stranger
            .set(&row(FILES_ROW), &word(true))
            .await
            .is_err()
    );
    assert!(grants(&rig).is_empty(), "nothing was granted");
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_the_account_in_the_rows_deletes_its_files_and_its_sign_in_here() {
    let mut rig = rig().await;
    rig.settings
        .set(&row(FILES_ROW), &word(true))
        .await
        .expect("on");
    rig.supervisor.tick().await;
    rig.graph.put_file("note.txt", b"from the drive");
    eventually("the file arrives", || local(&rig, "note.txt").exists()).await;

    rig.settings
        .set(
            &row("accounts.graph-acct.remove"),
            &toml::Value::Boolean(true),
        )
        .await
        .expect("remove");
    assert!(!local(&rig, "note.txt").exists(), "its files are gone");
    let service: &Service = &rig.hosted.service;
    assert!(service.registry().accounts.is_empty());
    assert!(service.registry().grants.is_empty());
}
