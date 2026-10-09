//! `Sync1.ConfirmDiscard` on a private bus: a real engine over an in-memory replica that is then
//! emptied is held (`Status` says so and `NeedsConfirmation` is signalled), nothing is removed
//! until a caller who may pause the dataset confirms, and then the files go and the hold is
//! cleared. Confirming with nothing held, or as a caller that cannot see the dataset, is refused.

use crate::common;

use common::bus::PrivateBus;
use common::{Known, NO_FITTING, client, error_name};
use porter_core::capability::{
    Access, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_core::{AppName, UnixSeconds};
use porter_dbus::{CallerRole, Details, SYNC_ERROR_NOTHING_HELD, SyncProxy};
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

const DATASET: &str = "a1/notes";
const OWNER: &str = "org.example.Notes";
const FILES: [&str; 3] = ["a.txt", "b.txt", "c.txt"];

/// A `NeedsConfirmation` signal as its dataset and details.
macro_rules! parts {
    () => {
        |signal| {
            let args = signal.args().expect("args");
            (args.dataset().to_string(), args.held().clone())
        }
    };
}

/// The replica the engine and the test both hold: the test is "someone else" writing to it.
/// The lock is held by the test while it makes changes a server would show at once: the engine's
/// listing waits for it.
#[derive(Debug, Clone)]
struct Shared(Arc<MemoryReplica>, Arc<tokio::sync::RwLock<()>>);

impl Shared {
    fn new(replica: MemoryReplica) -> Self {
        Self(Arc::new(replica), Arc::default())
    }
}

impl Replica for Shared {
    async fn changes(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
        let _still = self.1.read().await;
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
    let replica = Shared::new(MemoryReplica::new(storage(), 10, UnixSeconds(1)));
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

/// The numbers `Status` gives under `needs_confirmation`, while it is there.
async fn held_on_status(sync: &SyncProxy<'_>) -> Option<(u64, u64)> {
    let status = sync.status(DATASET).await.ok()?;
    let nested: std::collections::HashMap<String, zbus::zvariant::OwnedValue> = status
        .get("needs_confirmation")?
        .try_clone()
        .ok()?
        .try_into()
        .ok()?;
    Some((
        u64::try_from(&nested["discard"]).ok()?,
        u64::try_from(&nested["held"]).ok()?,
    ))
}

/// Three files synced down, then the replica forgets all of them and its history.
async fn emptied(rig: &Rig) {
    let mut put = Vec::new();
    for file in FILES {
        let item = PutItem {
            target: PutTarget::New(ItemPath(file.into())),
            content: Blob(file.as_bytes().to_vec()),
            hash: replica_hash(HashKind::Sha256, file.as_bytes()),
        };
        put.push(
            rig.replica
                .put(item, BaseVersion::Absent)
                .await
                .expect("put"),
        );
    }
    common::eventually("the files arrive", || {
        FILES.iter().all(|file| rig.dataset.get(file).is_some())
    })
    .await;
    // The engine lists between none of these: the removals and the lost history are one change
    // to it. A listing between two removals took them as plain deletions, so no hold came, or a
    // smaller one (rel-13 follow-up: the shell test waited 120 s for a hold that never came).
    let _still = rig.replica.1.write().await;
    for (id, version) in put {
        rig.replica
            .remove(&id, BaseVersion::At(version))
            .await
            .expect("remove");
    }
    rig.replica.0.compact();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_discard_is_signalled_stays_until_confirmed_and_then_the_files_go() {
    let rig = rig().await;
    let owner = client(&rig.bus, &rig.known, OWNER, CallerRole::App).await;
    let sync = proxy(&owner).await;
    let mut holds = sync.receive_needs_confirmation().await.expect("stream");
    // Calling joins the roster: the daemon tells only connections it has heard from.
    sync.datasets().await.expect("datasets");
    emptied(&rig).await;

    let started = next_hold(&mut holds, parts!()).await;
    assert_eq!(
        (
            u64::try_from(&started["discard"]).ok(),
            u64::try_from(&started["held"]).ok()
        ),
        (Some(3), Some(3))
    );
    until("Status says so", || async {
        held_on_status(&sync).await == Some((3, 3))
    })
    .await;
    // It stays held, with every file in place, for as long as nobody says.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(held_on_status(&sync).await, Some((3, 3)));
    assert!(FILES.iter().all(|file| rig.dataset.get(file).is_some()));

    // Settings may confirm (it may pause); a caller that cannot see the dataset may not.
    let stranger = client(&rig.bus, &rig.known, "org.example.Other", CallerRole::App).await;
    let agent = client(&rig.bus, &rig.known, "org.example.Agent", CallerRole::Agent).await;
    for who in [&stranger, &agent] {
        let err = proxy(who)
            .await
            .confirm_discard(DATASET)
            .await
            .expect_err("cannot see it");
        assert_eq!(error_name(&err), NO_FITTING);
    }
    assert_eq!(held_on_status(&sync).await, Some((3, 3)), "still held");

    let settings = client(
        &rig.bus,
        &rig.known,
        "org.quire.Settings",
        CallerRole::Settings,
    )
    .await;
    proxy(&settings)
        .await
        .confirm_discard(DATASET)
        .await
        .expect("confirm");
    let ended = next_hold(&mut holds, parts!()).await;
    assert!(ended.is_empty(), "the hold ended: {ended:?}");
    common::eventually("the files are gone", || {
        FILES.iter().all(|file| rig.dataset.get(file).is_none())
    })
    .await;
    assert_eq!(held_on_status(&sync).await, None);

    let err = sync
        .confirm_discard(DATASET)
        .await
        .expect_err("already confirmed");
    assert_eq!(error_name(&err), SYNC_ERROR_NOTHING_HELD);
}

#[tokio::test(flavor = "multi_thread")]
async fn confirming_with_nothing_held_is_refused_and_a_name_nothing_runs_under_is_missing() {
    let rig = rig().await;
    let owner = client(&rig.bus, &rig.known, OWNER, CallerRole::App).await;
    let sync = proxy(&owner).await;
    let err = sync
        .confirm_discard(DATASET)
        .await
        .expect_err("nothing held");
    assert_eq!(error_name(&err), SYNC_ERROR_NOTHING_HELD);
    let err = sync
        .confirm_discard("a1/nothing")
        .await
        .expect_err("no such dataset");
    assert_eq!(error_name(&err), NO_FITTING);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_lists_its_items_again_ends_the_hold_without_anyone_confirming() {
    let rig = rig().await;
    let owner = client(&rig.bus, &rig.known, OWNER, CallerRole::App).await;
    let sync = proxy(&owner).await;
    let mut holds = sync.receive_needs_confirmation().await.expect("stream");
    sync.datasets().await.expect("datasets");
    emptied(&rig).await;
    assert!(!next_hold(&mut holds, parts!()).await.is_empty());
    for file in FILES {
        let item = PutItem {
            target: PutTarget::New(ItemPath(file.into())),
            content: Blob(file.as_bytes().to_vec()),
            hash: replica_hash(HashKind::Sha256, file.as_bytes()),
        };
        rig.replica
            .put(item, BaseVersion::Absent)
            .await
            .expect("put");
    }
    assert!(next_hold(&mut holds, parts!()).await.is_empty());
    assert!(FILES.iter().all(|file| rig.dataset.get(file).is_some()));
    let err = sync
        .confirm_discard(DATASET)
        .await
        .expect_err("nothing held now");
    assert_eq!(error_name(&err), SYNC_ERROR_NOTHING_HELD);
}

const SHELL: &str = "org.quire.Sill";

/// Whether `stream` stays silent for a while (the daemon's signals to one connection arrive in
/// order, so a quarter of a second after the ones that did arrive is ample on a private bus).
async fn silent<S, T>(stream: &mut S) -> bool
where
    S: Stream<Item = T> + Unpin,
{
    tokio::time::timeout(
        Duration::from_millis(500),
        std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)),
    )
    .await
    .is_err()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shell_hears_a_hold_and_its_end_but_no_data_of_a_dataset_it_does_not_own() {
    let rig = rig().await;
    let owner = client(&rig.bus, &rig.known, OWNER, CallerRole::App).await;
    let owner_sync = proxy(&owner).await;
    let mut owner_progress = owner_sync.receive_progress().await.expect("stream");
    owner_sync.datasets().await.expect("datasets");
    let shell = client(&rig.bus, &rig.known, SHELL, CallerRole::SheetHost).await;
    let sync = proxy(&shell).await;
    let mut holds = sync.receive_needs_confirmation().await.expect("stream");
    let mut progress = sync.receive_progress().await.expect("stream");
    let mut conflicts = sync.receive_conflict().await.expect("stream");
    // The shell asks for no data: it only joins the ones that are told.
    sync.watch().await.expect("watch");
    assert!(sync.datasets().await.expect("datasets").is_empty());
    emptied(&rig).await;

    let started = next_hold(&mut holds, parts!()).await;
    assert_eq!(
        (
            u64::try_from(&started["discard"]).ok(),
            u64::try_from(&started["held"]).ok()
        ),
        (Some(3), Some(3))
    );
    let account = String::try_from(started["account"].try_clone().expect("clone")).expect("s");
    assert_eq!(account, "/org/quire/Accounts1/account/a1");

    // It may not read, steer or confirm what it does not own.
    let err = sync.status(DATASET).await.expect_err("not its dataset");
    assert_eq!(error_name(&err), NO_FITTING);
    for refused in [
        sync.pause(DATASET).await,
        sync.confirm_discard(DATASET).await,
    ] {
        assert_eq!(error_name(&refused.expect_err("refused")), NO_FITTING);
    }
    assert_eq!(
        held_on_status(&owner_sync).await,
        Some((3, 3)),
        "still held"
    );

    let settings = client(
        &rig.bus,
        &rig.known,
        "org.quire.Settings",
        CallerRole::Settings,
    )
    .await;
    proxy(&settings)
        .await
        .confirm_discard(DATASET)
        .await
        .expect("Settings confirms");
    let ended = next_hold(&mut holds, parts!()).await;
    assert!(ended.is_empty(), "the hold ended: {ended:?}");

    // The owner was told of the transfers (so the stream works); the shell was told of none.
    assert!(
        !silent(&mut owner_progress).await,
        "the owner hears progress"
    );
    assert!(silent(&mut progress).await, "no Progress for the shell");
    assert!(silent(&mut conflicts).await, "no Conflict for the shell");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_that_joins_after_the_hold_started_is_told_at_once() {
    let rig = rig().await;
    let owner = client(&rig.bus, &rig.known, OWNER, CallerRole::App).await;
    let owner_sync = proxy(&owner).await;
    let mut owner_holds = owner_sync
        .receive_needs_confirmation()
        .await
        .expect("stream");
    owner_sync.datasets().await.expect("datasets");
    emptied(&rig).await;
    assert!(!next_hold(&mut owner_holds, parts!()).await.is_empty());
    until("Status says so", || async {
        held_on_status(&owner_sync).await == Some((3, 3))
    })
    .await;

    let shell = client(&rig.bus, &rig.known, SHELL, CallerRole::SheetHost).await;
    let sync = proxy(&shell).await;
    let mut holds = sync.receive_needs_confirmation().await.expect("stream");
    sync.watch().await.expect("watch");
    let told = next_hold(&mut holds, parts!()).await;
    assert_eq!(
        (
            u64::try_from(&told["discard"]).ok(),
            u64::try_from(&told["held"]).ok()
        ),
        (Some(3), Some(3))
    );
    assert_eq!(
        String::try_from(told["account"].try_clone().expect("clone")).ok(),
        Some("/org/quire/Accounts1/account/a1".to_owned())
    );
    // A stranger that joins is told of nothing.
    let stranger = client(&rig.bus, &rig.known, "org.example.Other", CallerRole::App).await;
    let other = proxy(&stranger).await;
    let mut others = other.receive_needs_confirmation().await.expect("stream");
    other.watch().await.expect("watch");
    assert!(silent(&mut others).await, "a stranger is told no hold");
}

/// The details of the next `NeedsConfirmation` signal; `read` takes a signal apart into its
/// dataset and details.
async fn next_hold<S, T>(holds: &mut S, read: impl Fn(&T) -> (String, Details)) -> Details
where
    S: Stream<Item = T> + Unpin,
{
    let signal = tokio::time::timeout(
        porter_fake::GENEROUS,
        std::future::poll_fn(|cx| Pin::new(&mut *holds).poll_next(cx)),
    )
    .await
    .expect("a NeedsConfirmation signal within the generous wait")
    .expect("open stream");
    let (dataset, held) = read(&signal);
    assert_eq!(dataset, DATASET);
    held
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
