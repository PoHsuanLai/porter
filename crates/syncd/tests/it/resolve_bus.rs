//! `Sync1.Resolve` on a private bus: a real engine over an in-memory replica reaches a conflict,
//! the `Conflict` signal carries its number, and the owning app settles it either way. Who may
//! call, a bad `how` and a conflict that is gone are answered with their own errors.

use crate::common;

use common::bus::PrivateBus;
use common::{INVALID_ARGS, Known, NO_FITTING, client, error_name};
use porter_core::capability::{
    Access, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_core::{AppName, UnixSeconds};
use porter_dbus::{
    CONFLICT_KEY_NUMBER, CallerRole, RESOLVE_KEEP_LOCAL, RESOLVE_KEEP_REMOTE,
    SYNC_ERROR_NO_SUCH_CONFLICT, SyncProxy,
};
use porter_sync::{
    BaseVersion, Blob, ByteRange, ChangePage, ConflictRule, Cursor, ItemPath, MemoryReplica,
    PutItem, PutRefused, PutTarget, Quota, RemoteId, RemoteVersion, Replica, ReplicaError,
};
use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use syncd::clock::SystemClock;
use syncd::dataset::{MemoryDataset, replica_hash};
use syncd::driver::Driver;
use syncd::engine::Engine;
use syncd::journal::Journal;
use syncd::scheduler::{MeteredPolicy, Network, Settings};
use syncd::service::{Access as Visible, DatasetName, Hub, serve};
use tokio::sync::{Notify, watch};
use zbus::export::futures_core::Stream;

const DENIED: &str = "org.quire.Accounts1.Error.Denied";
const DATASET: &str = "a1/notes";
const OWNER: &str = "org.example.Notes";

/// The replica the engine and the test both hold: the test is "someone else" writing to it.
#[derive(Debug, Clone)]
struct Shared(Arc<MemoryReplica>);

impl Replica for Shared {
    async fn changes(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
        self.0.changes(from).await
    }
    async fn fetch(&self, item: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        self.0.fetch(item, range).await
    }
    async fn put(
        &self,
        item: PutItem,
        base: BaseVersion,
    ) -> Result<(RemoteId, RemoteVersion), PutRefused> {
        self.0.put(item, base).await
    }
    async fn remove(
        &self,
        item: &RemoteId,
        base: BaseVersion,
    ) -> Result<RemoteVersion, PutRefused> {
        self.0.remove(item, base).await
    }
    async fn quota(&self) -> Result<Quota, ReplicaError> {
        self.0.quota().await
    }
    fn features(&self) -> StorageCap {
        self.0.features()
    }
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

/// A running dataset owned by `OWNER`, and the callers around it.
struct Rig {
    bus: PrivateBus,
    known: Known,
    replica: Shared,
    dataset: Arc<MemoryDataset>,
    _server: zbus::Connection,
    running: tokio::task::JoinHandle<()>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.running.abort();
    }
}

async fn rig() -> Rig {
    let bus = PrivateBus::start();
    let known = Known::default();
    let hub = Hub::default();
    let server = bus.connect().await;
    serve(&server, hub.clone(), known.clone())
        .await
        .expect("serve");
    let replica = Shared(Arc::new(MemoryReplica::new(storage(), 10, UnixSeconds(1))));
    let dataset = Arc::new(MemoryDataset::new("notes", ConflictRule::ShowInApp));
    let journal = bus.scratch().join("journal");
    let engine = Engine::new(
        replica.clone(),
        Arc::clone(&dataset),
        Journal::open(&journal.join("notes.sqlite")).expect("journal"),
        SystemClock,
    );
    let owners = BTreeSet::from([AppName::parse(OWNER).expect("name")]);
    let handle = hub.register(
        DatasetName::parse(DATASET).expect("name"),
        Visible { owners },
    );
    let (_net, network) = watch::channel(Network::Unmetered);
    let driver = Driver::new(engine, handle, quick(), network, Arc::new(Notify::new()), 5);
    Rig {
        bus,
        known,
        replica,
        dataset,
        _server: server,
        running: tokio::spawn(driver.run()),
    }
}

async fn proxy(connection: &zbus::Connection) -> SyncProxy<'_> {
    SyncProxy::new(connection).await.expect("proxy")
}

async fn until<F: Future<Output = bool>>(what: &str, mut check: impl FnMut() -> F) {
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    deadline.fail(what);
}

async fn remote_text(replica: &Shared, id: &RemoteId) -> Option<String> {
    let Blob(bytes) = replica.fetch(id, ByteRange::Whole).await.ok()?;
    String::from_utf8(bytes).ok()
}

async fn conflicts_on_status(sync: &SyncProxy<'_>) -> Option<u64> {
    let status = sync.status(DATASET).await.ok()?;
    u64::try_from(status.get("conflicts")?).ok()
}

/// The owner's connection with a file that both sides have edited since they last agreed: the
/// `Conflict` signal's number is returned with the remote item.
async fn conflicted(rig: &Rig) -> (zbus::Connection, i64, RemoteId) {
    let owner = client(&rig.bus, &rig.known, OWNER, CallerRole::App).await;
    let sync = proxy(&owner).await;
    let mut conflicts = sync.receive_conflict().await.expect("stream");
    // Calling joins the roster: the daemon tells only connections it has heard from.
    sync.datasets().await.expect("datasets");

    let path = ItemPath("a.txt".into());
    let one = PutItem {
        target: PutTarget::New(path),
        content: Blob(b"one".to_vec()),
        hash: replica_hash(HashKind::Sha256, b"one"),
    };
    let (id, version) = rig
        .replica
        .put(one, BaseVersion::Absent)
        .await
        .expect("put");
    until("the file arrives", || async {
        rig.dataset.get("a.txt") == Some(b"one".to_vec())
    })
    .await;

    // Both sides edit within one pause, so no cycle sees one edit alone.
    sync.pause(DATASET).await.expect("pause");
    tokio::time::sleep(Duration::from_millis(400)).await;
    let theirs = PutItem {
        target: PutTarget::Existing(id.clone()),
        content: Blob(b"theirs".to_vec()),
        hash: replica_hash(HashKind::Sha256, b"theirs"),
    };
    rig.replica
        .put(theirs, BaseVersion::At(version))
        .await
        .expect("remote edit");
    rig.dataset.put("a.txt", b"mine");
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
    (owner, number, id)
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_remote_has_the_next_cycle_fetch_the_replicas_version_and_the_conflict_is_gone() {
    let rig = rig().await;
    let (owner, number, _id) = conflicted(&rig).await;
    let sync = proxy(&owner).await;
    assert_eq!(rig.dataset.get("a.txt"), Some(b"mine".to_vec()));
    assert_eq!(conflicts_on_status(&sync).await, Some(1));

    sync.resolve(DATASET, number, RESOLVE_KEEP_REMOTE)
        .await
        .expect("resolve");
    until("the replica's version is stored", || async {
        rig.dataset.get("a.txt") == Some(b"theirs".to_vec())
    })
    .await;
    until("no conflict is left", || async {
        conflicts_on_status(&sync).await == Some(0)
    })
    .await;

    let err = sync
        .resolve(DATASET, number, RESOLVE_KEEP_REMOTE)
        .await
        .expect_err("settled already");
    assert_eq!(error_name(&err), SYNC_ERROR_NO_SUCH_CONFLICT);
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_local_has_the_next_cycle_upload_the_local_content() {
    let rig = rig().await;
    let (owner, number, id) = conflicted(&rig).await;
    let sync = proxy(&owner).await;
    assert_eq!(
        remote_text(&rig.replica, &id).await.as_deref(),
        Some("theirs")
    );

    sync.resolve(DATASET, number, RESOLVE_KEEP_LOCAL)
        .await
        .expect("resolve");
    until("the local content is uploaded", || async {
        remote_text(&rig.replica, &id).await.as_deref() == Some("mine")
    })
    .await;
    until("no conflict is left", || async {
        conflicts_on_status(&sync).await == Some(0)
    })
    .await;
    assert_eq!(rig.dataset.get("a.txt"), Some(b"mine".to_vec()));

    let err = sync
        .resolve(DATASET, number, RESOLVE_KEEP_LOCAL)
        .await
        .expect_err("settled already");
    assert_eq!(error_name(&err), SYNC_ERROR_NO_SUCH_CONFLICT);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_how_is_invalid_args_naming_the_words_and_nothing_is_settled() {
    let rig = rig().await;
    let (owner, number, _id) = conflicted(&rig).await;
    let sync = proxy(&owner).await;
    for bad in ["", "keep", "KEEP_LOCAL", "keep_both", "local"] {
        let err = sync
            .resolve(DATASET, number, bad)
            .await
            .expect_err("bad how");
        assert_eq!(error_name(&err), INVALID_ARGS, "{bad:?}");
        let said = format!("{err}");
        assert!(
            said.contains(RESOLVE_KEEP_LOCAL) && said.contains(RESOLVE_KEEP_REMOTE),
            "{said}"
        );
    }
    assert_eq!(conflicts_on_status(&sync).await, Some(1));
    assert_eq!(rig.dataset.get("a.txt"), Some(b"mine".to_vec()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_conflict_that_was_never_stored_is_no_such_conflict() {
    let rig = rig().await;
    let owner = client(&rig.bus, &rig.known, OWNER, CallerRole::App).await;
    let sync = proxy(&owner).await;
    for number in [-1, 0, 1, 999] {
        let err = sync
            .resolve(DATASET, number, RESOLVE_KEEP_REMOTE)
            .await
            .expect_err("no conflict");
        assert_eq!(error_name(&err), SYNC_ERROR_NO_SUCH_CONFLICT, "{number}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_owning_app_settles_and_a_dataset_it_may_not_see_looks_missing() {
    let rig = rig().await;
    let (_owner, number, _id) = conflicted(&rig).await;
    let other = client(&rig.bus, &rig.known, "org.example.Other", CallerRole::App).await;
    let settings = client(
        &rig.bus,
        &rig.known,
        "org.quire.Settings",
        CallerRole::Settings,
    )
    .await;
    let agent = client(&rig.bus, &rig.known, "org.example.Agent", CallerRole::Agent).await;

    for (who, want) in [
        (&other, NO_FITTING),
        (&agent, NO_FITTING),
        (&settings, DENIED),
    ] {
        let err = proxy(who)
            .await
            .resolve(DATASET, number, RESOLVE_KEEP_REMOTE)
            .await
            .expect_err("not the owner");
        assert_eq!(error_name(&err), want);
    }
    // A name nothing runs under answers what an unseen one does, to everyone.
    for who in [&other, &settings] {
        let err = proxy(who)
            .await
            .resolve("a1/nothing", number, RESOLVE_KEEP_REMOTE)
            .await
            .expect_err("no such dataset");
        assert_eq!(error_name(&err), NO_FITTING);
    }
    let err = proxy(&other)
        .await
        .resolve("not a name", number, RESOLVE_KEEP_REMOTE)
        .await
        .expect_err("malformed");
    assert_eq!(error_name(&err), INVALID_ARGS);

    // None of it settled anything.
    let status = proxy(&settings)
        .await
        .status(DATASET)
        .await
        .expect("status");
    assert_eq!(u64::try_from(&status["conflicts"]).ok(), Some(1));
}
