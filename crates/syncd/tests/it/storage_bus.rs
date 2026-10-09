//! syncd's storage supervisor against the real accountd and a fake Graph drive on one private
//! bus. A Storage grant on a Microsoft account (class Files) becomes a dataset that mirrors the
//! app folder both ways; a two-sided edit is a `Conflict` signal with its number and `Resolve`
//! settles it; a grant that goes (or an account that is removed) drops the dataset. Photos runs
//! only behind its switch. The relay, not syncd, authenticates: the bearer is the fake
//! provider's, and the credential accountd holds appears nowhere.

use crate::common;

use accountd::{BusSheets, Options, TableCallers, serve_with};
use common::bus::PrivateBus;
use common::{Known, client};
use porter_client::{Accounts, DbusTransport};
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AppId, AppName, AuthKind, Candidate,
    CapabilityKind, Claim, Credential, DataClass, EndpointUrl, Family, GrantId, Isolation,
    LoginName, Offer, Provenance, Restriction, SecretKey, SecretPurpose, SecretText,
    ServiceEndpoint, SpaceScope, Subject, Tls, UnixSeconds,
};
use porter_dbus::{
    CONFLICT_KEY_NUMBER, Caller, CallerRole, RESOLVE_KEEP_REMOTE, SYNC_ERROR_NO_SUCH_CONFLICT,
    SyncProxy,
};
use porter_fake::{FakeProvider, FixedClock, MemoryStore, RecordingAudit};
use porter_fake_servers::{FakeGraph, GraphHandle, Running};
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use syncd::datasets::photos::PhotosSwitch;
use syncd::datasets::pim::{AccountdUnavailable, Wiring};
use syncd::datasets::storage::{
    ClientStorageGrants, FILES_APP, PHOTOS_APP, StorageConfig, StorageGrants, StorageKind,
    StorageSupervisor,
};
use syncd::paths::{AccountDir, Paths};
use syncd::scheduler::{MeteredPolicy, Network, Settings};
use syncd::service::{Access, Hub, serve};
use tokio::sync::watch;
use zbus::export::futures_core::Stream;

const ACCOUNT: &str = "graph-acct";
const SEGMENT: &str = "graph_acct";
const SYNCD: &str = "org.quire.Sync";
const BEARER: &str = "fake:graph-acct:graph";
const DATASET: &str = "graph_acct/storage_app_folder";

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

fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("app"),
        isolation: Isolation::Flatpak,
    }
}

fn quick() -> Settings {
    Settings {
        poll_base: 1,
        poll_max: 1,
        push_window: 0,
        batch_window: 0,
        metered: MeteredPolicy::Pause,
    }
}

/// The real grants, until the test hides them (a revoked grant, a removed account).
struct Gate {
    real: ClientStorageGrants<DbusTransport>,
    shown: Arc<AtomicBool>,
}

impl StorageGrants for Gate {
    async fn granted(&self, kind: StorageKind) -> Result<Vec<Candidate>, AccountdUnavailable> {
        match self.shown.load(Ordering::SeqCst) {
            true => self.real.granted(kind).await,
            false => Ok(Vec::new()),
        }
    }
}

struct Rig {
    bus: PrivateBus,
    known: Known,
    hub: Hub,
    graph: Running<GraphHandle>,
    paths: Paths,
    account: AccountDir,
    shown: Arc<AtomicBool>,
    supervisor: StorageSupervisor<DbusTransport, Gate>,
    _keep: Vec<zbus::Connection>,
    _network: watch::Sender<Network>,
}

fn grant(id: &str, class: DataClass) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("grant"),
        key: GrantKey {
            app: app(SYNCD),
            account: AccountId::parse(ACCOUNT).expect("id"),
            kind: CapabilityKind::Storage,
            class,
            usage: Usage::Background,
            space: SpaceScope::Any,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    }
}

async fn rig(photos: PhotosSwitch) -> Rig {
    let graph = FakeGraph::start(BEARER).await.expect("graph");
    graph.set_limit(1_000_000);
    let endpoint = EndpointUrl::parse(graph.base_url()).expect("url");
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
            url: endpoint,
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
    let service = Arc::new(
        AccountService::new(
            vec![provider],
            Registry {
                grants: vec![
                    grant("files-grant", DataClass::Files),
                    grant("photos-grant", DataClass::Photos),
                ],
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
    serve_with(&accountd, service, Arc::clone(&table), Options::default())
        .await
        .expect("accountd serves");
    let syncd_side = bus.connect().await;
    table.introduce_as(
        syncd_side.unique_name().expect("name").as_str(),
        Caller {
            app: app(SYNCD),
            role: CallerRole::App,
        },
    );
    let accounts = Arc::new(Accounts::over(DbusTransport::over(syncd_side.clone())));

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
    let shown = Arc::new(AtomicBool::new(true));
    let wiring = Wiring {
        accounts: Arc::clone(&accounts),
        hub: hub.clone(),
        paths: paths.clone(),
        settings: quick(),
        network,
        owners: Access::default(),
    };
    let supervisor = StorageSupervisor::new(
        wiring,
        Gate {
            real: ClientStorageGrants::new(accounts),
            shown: Arc::clone(&shown),
        },
        StorageConfig {
            photos,
            ..StorageConfig::default()
        },
    );
    Rig {
        bus,
        known,
        hub,
        graph,
        paths,
        account: AccountDir::parse(SEGMENT).expect("segment"),
        shown,
        supervisor,
        _keep: vec![accountd, syncd_side, server],
        _network: network_keeps,
    }
}

/// Polls every 20 ms for up to [`porter_fake::GENEROUS`] by the clock (a loaded machine runs a
/// cycle slowly).
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

async fn names(sync: &SyncProxy<'_>) -> Vec<String> {
    sync.datasets().await.expect("datasets")
}

fn local(rig: &Rig, rel: &str) -> std::path::PathBuf {
    rig.paths.storage_dir(&rig.account).join(rel)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_files_grant_on_a_graph_account_mirrors_the_app_folder_both_ways_through_the_relay() {
    let mut rig = rig(PhotosSwitch::Off).await;
    let files = client(&rig.bus, &rig.known, FILES_APP, CallerRole::App).await;
    let sync = SyncProxy::new(&files).await.expect("proxy");
    assert_eq!(names(&sync).await, Vec::<String>::new(), "nothing before");

    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, [DATASET], "the grant became a dataset");
    // The Photos switch is off, so the Photos-class grant starts nothing.
    let photos = client(&rig.bus, &rig.known, PHOTOS_APP, CallerRole::App).await;
    assert_eq!(
        names(&SyncProxy::new(&photos).await.expect("proxy")).await,
        Vec::<String>::new()
    );

    // Up, in a folder, and down.
    std::fs::create_dir_all(local(&rig, "notes")).expect("dir");
    std::fs::write(local(&rig, "notes/a.txt"), b"from here").expect("write");
    eventually("the file is on the drive", || {
        rig.graph.file("notes/a.txt").as_deref() == Some(b"from here".as_slice())
    })
    .await;
    rig.graph.put_file("from-phone/b.txt", b"from there");
    eventually("the remote file arrives", || {
        std::fs::read(local(&rig, "from-phone/b.txt"))
            .ok()
            .as_deref()
            == Some(b"from there".as_slice())
    })
    .await;
    rig.graph.delete("notes/a.txt");
    eventually("a remote delete is a local delete", || {
        !local(&rig, "notes/a.txt").exists()
    })
    .await;

    // The relay authenticated every request; the credential accountd holds is nowhere.
    let want = format!("Bearer {BEARER}");
    let hits = rig.graph.hits();
    assert!(!hits.is_empty());
    assert!(
        hits.iter()
            .all(|h| h.authorization.as_deref() == Some(want.as_str()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_two_sided_edit_is_a_conflict_signal_with_a_number_and_keep_remote_settles_it() {
    let mut rig = rig(PhotosSwitch::Off).await;
    let files = client(&rig.bus, &rig.known, FILES_APP, CallerRole::App).await;
    let sync = SyncProxy::new(&files).await.expect("proxy");
    let mut conflicts = sync.receive_conflict().await.expect("stream");
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, [DATASET]);

    rig.graph.put_file("doc.txt", b"one");
    eventually("the file arrives", || {
        std::fs::read(local(&rig, "doc.txt")).ok().as_deref() == Some(b"one".as_slice())
    })
    .await;

    // Both sides edit within one pause, so no cycle sees one edit alone.
    sync.pause(DATASET).await.expect("pause");
    tokio::time::sleep(Duration::from_millis(400)).await;
    rig.graph.put_file("doc.txt", b"theirs");
    std::fs::write(local(&rig, "doc.txt"), b"mine").expect("local edit");
    sync.resume(DATASET).await.expect("resume");

    let signal = tokio::time::timeout(
        Duration::from_secs(120),
        std::future::poll_fn(|cx| Pin::new(&mut conflicts).poll_next(cx)),
    )
    .await
    .expect("a Conflict signal in time")
    .expect("open stream");
    let args = signal.args().expect("args");
    assert_eq!(args.dataset(), &DATASET);
    let number = i64::try_from(&args.conflict()[CONFLICT_KEY_NUMBER]).expect("number is an x");
    assert!(number >= 1, "the journal's own number: {number}");

    sync.resolve(DATASET, number, RESOLVE_KEEP_REMOTE)
        .await
        .expect("resolve");
    eventually("the drive's version is stored locally", || {
        std::fs::read(local(&rig, "doc.txt")).ok().as_deref() == Some(b"theirs".as_slice())
    })
    .await;
    let err = sync
        .resolve(DATASET, number, RESOLVE_KEEP_REMOTE)
        .await
        .expect_err("settled already");
    assert!(
        format!("{err:?}").contains(SYNC_ERROR_NO_SUCH_CONFLICT),
        "{err:?}"
    );
    assert_eq!(
        rig.graph.file("doc.txt").as_deref(),
        Some(b"theirs".as_slice())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_grant_drops_the_dataset_keeps_the_files_and_a_removed_account_wipes_them() {
    let mut rig = rig(PhotosSwitch::Off).await;
    let files = client(&rig.bus, &rig.known, FILES_APP, CallerRole::App).await;
    let sync = SyncProxy::new(&files).await.expect("proxy");
    rig.supervisor.tick().await;
    rig.graph.put_file("keep.txt", b"kept");
    eventually("the file arrives", || local(&rig, "keep.txt").exists()).await;

    // A tick that cannot reach accountd, or that finds the grant, changes nothing.
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, [DATASET]);

    rig.shown.store(false, Ordering::SeqCst);
    rig.supervisor.tick().await;
    assert_eq!(
        names(&sync).await,
        Vec::<String>::new(),
        "the grant is gone"
    );
    assert!(rig.supervisor.datasets().is_empty());
    assert!(
        local(&rig, "keep.txt").exists(),
        "the files are the person's"
    );

    // The engine is stopped: a remote change no longer arrives.
    rig.graph.put_file("late.txt", b"late");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!local(&rig, "late.txt").exists());

    // The grant comes back: the folder is picked up again where it was.
    rig.shown.store(true, Ordering::SeqCst);
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, [DATASET]);
    eventually("the missed file arrives", || {
        local(&rig, "late.txt").exists()
    })
    .await;

    // AccountRemoved: the wipe takes the folder and the journal with it.
    rig.shown.store(false, Ordering::SeqCst);
    syncd::removal::wipe(&rig.paths, &rig.hub, &rig.account)
        .await
        .expect("wipe");
    rig.supervisor.tick().await;
    assert!(!rig.paths.storage_dir(&rig.account).exists());
    assert!(!rig.paths.journals.join(SEGMENT).exists());
    assert_eq!(names(&sync).await, Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn photos_run_only_behind_the_switch_for_a_photos_grant_and_an_import_reaches_the_drive() {
    let mut rig = rig(PhotosSwitch::On).await;
    let photos = client(&rig.bus, &rig.known, PHOTOS_APP, CallerRole::App).await;
    let sync = SyncProxy::new(&photos).await.expect("proxy");
    rig.supervisor.tick().await;
    let mut seen = names(&sync).await;
    seen.sort();
    assert_eq!(
        seen,
        ["graph_acct/photos_metadata", "graph_acct/photos_originals"]
    );
    // The app folder dataset is the Files app's, not the Photos app's.
    let files = client(&rig.bus, &rig.known, FILES_APP, CallerRole::App).await;
    assert_eq!(
        names(&SyncProxy::new(&files).await.expect("proxy")).await,
        [DATASET]
    );

    let library = rig
        .supervisor
        .photos_library(&rig.account)
        .expect("a library while Photos runs")
        .clone();
    let source = rig.bus.scratch().join("IMG_1.jpg");
    std::fs::write(&source, b"pixels").expect("photo");
    let imported = library.import(&source).expect("import");
    eventually("the original is on the drive, by content", || {
        let shard = &imported.id.rel();
        rig.graph
            .file(&format!("Photos/Originals/{shard}"))
            .as_deref()
            == Some(b"pixels".as_slice())
    })
    .await;

    rig.shown.store(false, Ordering::SeqCst);
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, Vec::<String>::new());
}
