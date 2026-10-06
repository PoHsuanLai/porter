//! `org.quire.Sync1` on a private bus: who may call, what a name that has no dataset answers,
//! `Status` with its quota, the pause switch, the unicast signals, and the introspection the
//! frozen XML pins. One test runs a real engine over `MemoryReplica` and reads it over the bus.

mod common;

use common::bus::PrivateBus;
use common::{ACCESS_DENIED, INVALID_ARGS, Known, NO_FITTING, client, error_name, eventually};
use porter_core::capability::{
    Access, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_core::{Bytes, UnixSeconds};
use porter_dbus::{CallerRole, STATUS_KEY_QUOTA, SYNC_BUS, SYNC_PATH, SyncProxy};
use porter_sync::{
    BaseVersion, Conflict, ConflictRule, LocalId, MemoryReplica, Quota, RemoteId, RemoteSide,
    RemoteVersion, StoredConflict,
};
use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use syncd::clock::SystemClock;
use syncd::dataset::MemoryDataset;
use syncd::driver::Driver;
use syncd::engine::Engine;
use syncd::journal::Journal;
use syncd::scheduler::{MeteredPolicy, Network, Settings};
use syncd::service::{Access as Visible, DatasetName, Event, Hub, StatusSnapshot, serve};
use tokio::sync::{Notify, watch};
use zbus::export::futures_core::Stream;
use zbus::zvariant::OwnedValue;

struct Rig {
    bus: PrivateBus,
    known: Known,
    hub: Hub,
    _server: zbus::Connection,
}

async fn rig() -> Rig {
    let bus = PrivateBus::start();
    let known = Known::default();
    let hub = Hub::default();
    let server = bus.connect().await;
    serve(&server, hub.clone(), known.clone())
        .await
        .expect("serve");
    Rig {
        bus,
        known,
        hub,
        _server: server,
    }
}

fn name(text: &str) -> DatasetName {
    DatasetName::parse(text).expect("dataset name")
}

fn owned_by(app: &str) -> Visible {
    Visible {
        owners: BTreeSet::from([porter_core::AppName::parse(app).expect("name")]),
    }
}

async fn proxy(connection: &zbus::Connection) -> SyncProxy<'_> {
    SyncProxy::new(connection).await.expect("proxy")
}

fn interface_of(xml: &str) -> Vec<String> {
    xml.lines()
        .map(str::trim)
        .skip_while(|l| !l.starts_with("<interface name=\"org.quire.Sync1\""))
        .take_while(|l| *l != "</interface>")
        .map(str::to_owned)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_served_object_has_exactly_the_frozen_interface() {
    let rig = rig().await;
    let me = client(&rig.bus, &rig.known, "org.example.App", CallerRole::App).await;
    let live = zbus::fdo::IntrospectableProxy::builder(&me)
        .destination(SYNC_BUS)
        .expect("destination")
        .path(SYNC_PATH)
        .expect("path")
        .build()
        .await
        .expect("proxy")
        .introspect()
        .await
        .expect("introspect");
    let frozen = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dbus/org.quire.Sync1.xml"),
    )
    .expect("the checked-in xml");
    let (served, pinned) = (interface_of(&live), interface_of(&frozen));
    assert!(served.len() > 10, "{live}");
    assert_eq!(served, pinned);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sender_nothing_names_is_access_denied_and_a_named_one_may_ask() {
    let rig = rig().await;
    let stranger = rig.bus.connect().await;
    let err = proxy(&stranger)
        .await
        .datasets()
        .await
        .expect_err("unknown caller");
    assert_eq!(error_name(&err), ACCESS_DENIED);
    let known = client(&rig.bus, &rig.known, "org.example.App", CallerRole::App).await;
    assert_eq!(
        proxy(&known).await.datasets().await.expect("datasets"),
        Vec::<String>::new()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_dataset_every_name_answers_the_refusal_and_a_bad_name_is_invalid() {
    let rig = rig().await;
    let me = client(&rig.bus, &rig.known, "org.example.App", CallerRole::App).await;
    let sync = proxy(&me).await;
    assert!(sync.datasets().await.expect("datasets").is_empty());
    for dataset in ["acct/pim", "a1/photos_originals"] {
        for err in [
            sync.status(dataset).await.map(drop).expect_err("status"),
            sync.pause(dataset).await.expect_err("pause"),
            sync.resume(dataset).await.expect_err("resume"),
        ] {
            assert_eq!(error_name(&err), NO_FITTING, "{dataset}");
        }
    }
    for bad in ["", "pim", "a/b/c", "../x/y", "A/B"] {
        let err = sync.status(bad).await.map(drop).expect_err("bad name");
        assert_eq!(error_name(&err), INVALID_ARGS, "{bad:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_sees_what_it_owns_settings_sees_all_and_status_carries_quota_and_the_pause() {
    let rig = rig().await;
    let photos = rig
        .hub
        .register(name("a1/photos_originals"), owned_by("org.quire.Photos"));
    let _pim = rig.hub.register(name("a1/pim"), owned_by("org.quire.Sill"));
    photos.publish(StatusSnapshot {
        anchor_age: Some(12),
        pending: 3,
        conflicts: 1,
        quota: Some(Quota {
            used: Bytes(10),
            total: Some(Bytes(100)),
        }),
        ..StatusSnapshot::default()
    });
    let app = client(&rig.bus, &rig.known, "org.quire.Photos", CallerRole::App).await;
    let settings = client(
        &rig.bus,
        &rig.known,
        "org.quire.Settings",
        CallerRole::Settings,
    )
    .await;
    assert_eq!(
        proxy(&app).await.datasets().await.expect("datasets"),
        ["a1/photos_originals"]
    );
    assert_eq!(
        proxy(&settings).await.datasets().await.expect("datasets"),
        ["a1/photos_originals", "a1/pim"]
    );

    let status = proxy(&app)
        .await
        .status("a1/photos_originals")
        .await
        .expect("status");
    assert_eq!(u64::try_from(&status["pending"]).ok(), Some(3));
    assert_eq!(u64::try_from(&status["conflicts"]).ok(), Some(1));
    assert_eq!(i64::try_from(&status["anchor_age"]).ok(), Some(12));
    assert_eq!(bool::try_from(&status["paused"]).ok(), Some(false));
    let quota: std::collections::HashMap<String, OwnedValue> = status[STATUS_KEY_QUOTA]
        .try_clone()
        .expect("clone")
        .try_into()
        .expect("a{sv}");
    assert_eq!(u64::try_from(&quota["used"]).ok(), Some(10));
    assert_eq!(u64::try_from(&quota["total"]).ok(), Some(100));

    let err = proxy(&app)
        .await
        .status("a1/pim")
        .await
        .map(drop)
        .expect_err("not theirs");
    assert_eq!(
        error_name(&err),
        NO_FITTING,
        "an invisible dataset looks like none"
    );
    let err = proxy(&app)
        .await
        .pause("a1/pim")
        .await
        .expect_err("not theirs");
    assert_eq!(error_name(&err), NO_FITTING);

    proxy(&app)
        .await
        .pause("a1/photos_originals")
        .await
        .expect("pause");
    assert_eq!(photos.pausing(), syncd::scheduler::Pausing::Paused);
    let status = proxy(&settings)
        .await
        .status("a1/photos_originals")
        .await
        .expect("status");
    assert_eq!(bool::try_from(&status["paused"]).ok(), Some(true));
    proxy(&settings)
        .await
        .resume("a1/photos_originals")
        .await
        .expect("resume");
    assert_eq!(photos.pausing(), syncd::scheduler::Pausing::Running);
}

async fn next<S: Stream + Unpin>(stream: &mut S) -> S::Item {
    tokio::time::timeout(
        Duration::from_secs(5),
        std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)),
    )
    .await
    .expect("a signal in time")
    .expect("the stream is open")
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_and_conflict_go_only_to_callers_who_may_see_the_dataset() {
    let rig = rig().await;
    let photos = rig
        .hub
        .register(name("a1/photos_originals"), owned_by("org.quire.Photos"));
    let pim = rig.hub.register(name("a1/pim"), owned_by("org.quire.Sill"));
    let app = client(&rig.bus, &rig.known, "org.quire.Photos", CallerRole::App).await;
    let sill = client(&rig.bus, &rig.known, "org.quire.Sill", CallerRole::App).await;
    let (app_proxy, sill_proxy) = (proxy(&app).await, proxy(&sill).await);
    let mut app_progress = app_proxy.receive_progress().await.expect("stream");
    let mut sill_progress = sill_proxy.receive_progress().await.expect("stream");
    let mut app_conflict = app_proxy.receive_conflict().await.expect("stream");
    // Calling joins the roster: the daemon tells only connections it has heard from.
    app_proxy.datasets().await.expect("datasets");
    sill_proxy.datasets().await.expect("datasets");

    photos.tell(Event::Progress {
        dataset: name("a1/photos_originals"),
        fetched: 2,
        uploaded: 1,
    });
    pim.tell(Event::Progress {
        dataset: name("a1/pim"),
        fetched: 7,
        uploaded: 0,
    });
    photos.tell(Event::Conflict {
        dataset: name("a1/photos_originals"),
        conflict: Box::new(StoredConflict {
            number: Some(1),
            conflict: Conflict {
                item: RemoteId("r1".into()),
                base: BaseVersion::Absent,
                remote: RemoteSide::Exists(RemoteId("r1".into()), RemoteVersion("v2".into())),
            },
            local: LocalId("IMG_1.HEIC".into()),
            local_hash: None,
            at: UnixSeconds(5),
        }),
    });

    let signal = next(&mut app_progress).await;
    assert_eq!(
        signal.args().expect("args").dataset(),
        &"a1/photos_originals"
    );
    let signal = next(&mut sill_progress).await;
    assert_eq!(
        signal.args().expect("args").dataset(),
        &"a1/pim",
        "Sill never hears of the Photos dataset"
    );
    let conflict = next(&mut app_conflict).await;
    let args = conflict.args().expect("args");
    assert_eq!(args.dataset(), &"a1/photos_originals");
    assert!(args.conflict().contains_key("remote"));
}

fn storage() -> StorageCap {
    StorageCap {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        quota: QuotaReport::Reported,
        scope: StorageScope::AppFolder,
        hashes: HashKind::Sha256,
        ranges: Offered::Present,
        chunked_upload: Offered::Absent,
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

async fn used(sync: &SyncProxy<'_>, dataset: &str) -> Option<u64> {
    let status = sync.status(dataset).await.ok()?;
    let quota: std::collections::HashMap<String, OwnedValue> = status
        .get(STATUS_KEY_QUOTA)?
        .try_clone()
        .ok()?
        .try_into()
        .ok()?;
    u64::try_from(&quota["used"]).ok()
}

/// Polls `Status` until the replica's used bytes are `want` (a minute at most: the journal
/// syncs to disk on every write, and a loaded machine is slow); the last reading.
async fn used_reaches(sync: &SyncProxy<'_>, dataset: &str, want: u64) -> Option<u64> {
    let mut seen = None;
    for _ in 0..3000 {
        seen = used(sync, dataset).await;
        if seen == Some(want) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    seen
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_engine_syncs_to_its_replica_and_the_bus_shows_it_until_it_is_paused() {
    let rig = rig().await;
    let dir = rig.bus.scratch().join("journal");
    let dataset = Arc::new(MemoryDataset::new(
        "photos_originals",
        ConflictRule::Impossible,
    ));
    let engine = Engine::new(
        MemoryReplica::new(storage(), 10, UnixSeconds(1)),
        Arc::clone(&dataset),
        Journal::open(&dir.join("photos_originals.sqlite")).expect("journal"),
        SystemClock,
    );
    let handle = rig
        .hub
        .register(name("a1/photos_originals"), owned_by("org.quire.Photos"));
    let (_net, network) = watch::channel(Network::Unmetered);
    let driver = Driver::new(engine, handle, quick(), network, Arc::new(Notify::new()), 7);
    let running = tokio::spawn(driver.run());

    let app = client(&rig.bus, &rig.known, "org.quire.Photos", CallerRole::App).await;
    let sync = proxy(&app).await;
    dataset.put("IMG_1.HEIC", b"abc");
    let seen = used_reaches(&sync, "a1/photos_originals", 3).await;
    assert_eq!(
        seen,
        Some(3),
        "the upload shows as the replica's used bytes"
    );
    let status = sync.status("a1/photos_originals").await.expect("status");
    assert_eq!(u64::try_from(&status["pending"]).ok(), Some(0));
    assert!(status.contains_key("anchor_age"));

    sync.pause("a1/photos_originals").await.expect("pause");
    tokio::time::sleep(Duration::from_millis(100)).await;
    dataset.put("IMG_2.HEIC", b"defg");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        used(&sync, "a1/photos_originals").await,
        Some(3),
        "paused: nothing went up"
    );
    sync.resume("a1/photos_originals").await.expect("resume");
    assert_eq!(used_reaches(&sync, "a1/photos_originals", 7).await, Some(7));

    // Removing the account's datasets from the hub ends the driver.
    rig.hub
        .forget_account(&syncd::paths::AccountDir::parse("a1").expect("account"));
    eventually("the driver stops", || running.is_finished()).await;
}
