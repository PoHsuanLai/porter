//! The drive listing (`syncd::datasets::drive`) against the fake Graph drive and the fake Google
//! My Drive, over loopback with the bearer added as the relay adds it: a whole drive listed into
//! its journal and read back by path and id, a pass cut short and resumed, a restart that keeps
//! the tree, a folder moved, an anchor that expired, and access not given yet.

use porter_capability::StorageCap;
use porter_fake_servers::{FakeGoogle, FakeGraph};
use porter_http::http::header::{AUTHORIZATION, HeaderValue};
use porter_http::http::{Request, Response};
use porter_http::{Http, HttpError};
use porter_http_reqwest::ReqwestHttp;
use porter_ids::{UnixSeconds, WebUrl};
use porter_sync::{
    Blob, ByteRange, ChangePage, Cursor, Folders, ItemKind, ItemPath, PutItem, PutRefused, Quota,
    RemoteId, RemoteVersion, Replica, ReplicaError, RetryAfter,
};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use syncd::datasets::drive::{DriveJournal, DriveReplica, Listed, list_once};
use test_kit::Scratch;

const TOKEN: &str = "ey.fake-access-token";
const NOW: UnixSeconds = UnixSeconds(1_000);

/// Adds the bearer to every request, as the relay does on the far side.
#[derive(Debug)]
struct Bearer(ReqwestHttp);

impl Http for Bearer {
    fn send(
        &self,
        mut request: Request<Vec<u8>>,
    ) -> impl Future<Output = Result<Response<Vec<u8>>, HttpError>> + Send {
        let mut value = HeaderValue::from_str(&format!("Bearer {TOKEN}")).expect("a value");
        value.set_sensitive(true);
        request.headers_mut().insert(AUTHORIZATION, value);
        self.0.send(request)
    }
}

/// A relay host that refuses every connection for want of access not given yet.
#[derive(Debug)]
struct NotGranted;

impl Http for NotGranted {
    async fn send(&self, _: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, HttpError> {
        Err(HttpError::NotGranted)
    }
}

type Graph = storage_graph::GraphReplica<Bearer>;
type Google = storage_gdrive::GdriveReplica<Bearer>;

fn graph(base: &str, page: usize, folders: Folders) -> Graph {
    let base = WebUrl::parse(base).expect("url");
    storage_graph::GraphReplica::new(
        Bearer(ReqwestHttp::new()),
        &base,
        "",
        porter_sync::Clock::fixed(NOW),
    )
    .with_root(storage_graph::Root::Drive)
    .with_page(page)
    .with_folders(folders)
}

fn google(base: &str, page: usize, folders: Folders) -> Google {
    let base = WebUrl::parse(base).expect("url");
    storage_gdrive::GdriveReplica::new(
        Bearer(ReqwestHttp::new()),
        &base,
        "",
        porter_sync::Clock::fixed(NOW),
    )
    .with_root(storage_gdrive::Root::Drive)
    .with_page(page)
    .with_folders(folders)
}

/// A replica whose feed answers `pages` pages, then asks to be tried again: a pass cut short
/// (the network went, the process stopped).
struct Cut<R> {
    inner: R,
    pages: AtomicUsize,
}

impl<R: Replica> Replica for Cut<R> {
    async fn changes(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
        let left = self.pages.load(Ordering::SeqCst);
        if left == 0 {
            return Err(ReplicaError::Transient(RetryAfter(5)));
        }
        self.pages.store(left - 1, Ordering::SeqCst);
        self.inner.changes(from).await
    }

    async fn fetch(&self, item: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        self.inner.fetch(item, range).await
    }

    async fn put(
        &self,
        item: PutItem,
        base: porter_sync::BaseVersion,
    ) -> Result<(RemoteId, RemoteVersion), PutRefused> {
        self.inner.put(item, base).await
    }

    async fn remove(
        &self,
        item: &RemoteId,
        base: porter_sync::BaseVersion,
    ) -> Result<RemoteVersion, PutRefused> {
        self.inner.remove(item, base).await
    }

    async fn quota(&self) -> Result<Quota, ReplicaError> {
        self.inner.quota().await
    }

    fn features(&self) -> StorageCap {
        self.inner.features()
    }
}

impl<R: DriveReplica> DriveReplica for Cut<R> {
    fn folders(&self) -> Folders {
        self.inner.folders()
    }
}

fn path(text: &str) -> ItemPath {
    ItemPath(text.into())
}

/// The names directly inside `at`, and whether each is a folder.
fn names(journal: &DriveJournal, at: &str) -> Vec<(String, bool)> {
    journal
        .children(&path(at))
        .expect("children")
        .into_iter()
        .map(|e| (e.name, e.kind == ItemKind::Folder))
        .collect()
}

/// A drive with a folder tree and files at two depths.
fn plant(graph: &porter_fake_servers::Running<porter_fake_servers::GraphHandle>) {
    graph.put_root_file("Docs/a.txt", b"a");
    graph.put_root_file("Docs/Sub/b.txt", b"bb");
    graph.put_root_file("c.txt", b"ccc");
}

#[tokio::test]
async fn a_whole_drive_is_listed_into_its_journal_and_read_by_path_and_id() {
    let scratch = Scratch::new("drive-listed");
    let fake = FakeGraph::start(TOKEN).await.expect("fake graph");
    plant(&fake);
    let journal = DriveJournal::open(&scratch.path().join("storage_drive.sqlite")).expect("open");
    let replica = graph(fake.base_url(), 2, journal.folders().expect("tree"));
    let pass = list_once(&journal, &replica).await.expect("pass");
    assert_eq!(pass.outcome, Listed::Done);
    assert!(pass.pages > 1, "{pass:?}");
    assert!(
        names(&journal, "").contains(&("Docs".to_owned(), true)),
        "{:?}",
        names(&journal, "")
    );
    assert!(names(&journal, "").contains(&("c.txt".to_owned(), false)));
    assert_eq!(
        names(&journal, "Docs"),
        [("Sub".to_owned(), true), ("a.txt".to_owned(), false)]
    );
    let b = journal
        .entry_at(&path("Docs/Sub/b.txt"))
        .expect("read")
        .expect("b.txt is listed");
    assert_eq!(
        (b.name.as_str(), b.size.0, b.kind),
        ("b.txt", 2, ItemKind::File)
    );
    let by_id = journal.entry(&b.id).expect("read").expect("by id");
    assert_eq!(by_id.path, path("Docs/Sub/b.txt"));
    let sub = journal
        .entry_at(&path("Docs/Sub"))
        .expect("read")
        .expect("Sub");
    assert_eq!(b.parent.as_ref(), Some(&sub.id));
    assert_eq!(
        journal.entry_at(&path("Docs/none.txt")).expect("read"),
        None
    );
    assert!(journal.children(&path("c.txt")).expect("read").is_empty());
    // Listing only: no item's content was asked for.
    assert!(
        !fake.hits().iter().any(|h| h.target.ends_with("/content")),
        "nothing is downloaded"
    );
}

#[tokio::test]
async fn a_pass_cut_short_resumes_and_a_restart_keeps_the_tree_and_the_anchor() {
    let scratch = Scratch::new("drive-resume");
    let fake = FakeGraph::start(TOKEN).await.expect("fake graph");
    plant(&fake);
    let file = scratch.path().join("storage_drive.sqlite");
    {
        let journal = DriveJournal::open(&file).expect("open");
        let cut = Cut {
            inner: graph(fake.base_url(), 2, journal.folders().expect("tree")),
            pages: AtomicUsize::new(1),
        };
        let pass = list_once(&journal, &cut).await.expect("pass");
        assert_eq!(
            (pass.outcome, pass.pages),
            (Listed::Retry(RetryAfter(5)), 1)
        );
        assert!(journal.listing().expect("listing"), "still listing");
    }
    // A new process: the journal again, a replica from the tree it kept.
    let journal = DriveJournal::open(&file).expect("open again");
    assert!(!journal.folders().expect("tree").is_empty());
    let pass = list_once(
        &journal,
        &graph(fake.base_url(), 2, journal.folders().expect("tree")),
    )
    .await
    .expect("pass");
    assert_eq!(pass.outcome, Listed::Done);
    assert!(!pass.relisted);
    assert!(!journal.listing().expect("listing"), "the listing ended");
    assert!(
        journal
            .entry_at(&path("Docs/Sub/b.txt"))
            .expect("read")
            .is_some()
    );
    let listed = journal.len().expect("len");

    // A file added: a restart reads only what changed, and asks for no folder by id (the tree
    // was kept); the drive's root it reads once, to know where paths start.
    fake.put_root_file("Docs/Sub/new.txt", b"n");
    let journal = DriveJournal::open(&file).expect("open a third time");
    let before = fake.hits().len();
    let pass = list_once(
        &journal,
        &graph(fake.base_url(), 50, journal.folders().expect("tree")),
    )
    .await
    .expect("pass");
    assert_eq!(pass.outcome, Listed::Done);
    assert!(pass.upserted <= 3, "{pass:?}");
    assert_eq!(journal.len().expect("len"), listed + 1);
    let asked: Vec<String> = fake.hits()[before..]
        .iter()
        .map(|h| h.target.clone())
        .collect();
    assert!(
        !asked.iter().any(|t| t.contains("/items/")),
        "no folder is asked for again: {asked:?}"
    );
    assert_eq!(
        journal
            .entry_at(&path("Docs/Sub/new.txt"))
            .expect("read")
            .map(|e| e.size.0),
        Some(1)
    );
}

#[tokio::test]
async fn a_folder_moved_re_paths_what_it_holds() {
    let scratch = Scratch::new("drive-move");
    let fake = FakeGraph::start(TOKEN).await.expect("fake graph");
    plant(&fake);
    let journal = DriveJournal::open(&scratch.path().join("storage_drive.sqlite")).expect("open");
    let replica = graph(fake.base_url(), 50, journal.folders().expect("tree"));
    list_once(&journal, &replica).await.expect("pass");
    let b = journal
        .entry_at(&path("Docs/Sub/b.txt"))
        .expect("read")
        .expect("b.txt");
    assert!(fake.move_root_item("Docs", "Archive/Old Docs"));
    let pass = list_once(&journal, &replica).await.expect("pass");
    assert_eq!(pass.outcome, Listed::Done);
    assert_eq!(
        journal.entry_at(&path("Docs/Sub/b.txt")).expect("read"),
        None
    );
    let moved = journal.entry(&b.id).expect("read").expect("the same file");
    assert_eq!(moved.path, path("Archive/Old Docs/Sub/b.txt"));
    assert_eq!(
        names(&journal, "Archive/Old Docs"),
        [("Sub".to_owned(), true), ("a.txt".to_owned(), false)]
    );
}

#[tokio::test]
async fn an_expired_anchor_lists_again_and_removes_what_the_new_listing_lacks() {
    let scratch = Scratch::new("drive-expired");
    let fake = FakeGraph::start(TOKEN).await.expect("fake graph");
    plant(&fake);
    let journal = DriveJournal::open(&scratch.path().join("storage_drive.sqlite")).expect("open");
    let replica = graph(fake.base_url(), 2, journal.folders().expect("tree"));
    list_once(&journal, &replica).await.expect("pass");
    assert!(journal.entry_at(&path("c.txt")).expect("read").is_some());
    fake.delete_root_item("c.txt");
    fake.delete_root_item("Docs/Sub");
    fake.put_root_file("d.txt", b"d");
    fake.expire_delta_tokens();
    let pass = list_once(&journal, &replica).await.expect("pass");
    assert_eq!(pass.outcome, Listed::Done);
    assert!(pass.relisted);
    assert!(pass.removed >= 3, "{pass:?}");
    assert_eq!(journal.entry_at(&path("c.txt")).expect("read"), None);
    assert_eq!(
        journal.entry_at(&path("Docs/Sub/b.txt")).expect("read"),
        None
    );
    assert!(
        journal
            .entry_at(&path("Docs/a.txt"))
            .expect("read")
            .is_some()
    );
    assert!(journal.entry_at(&path("d.txt")).expect("read").is_some());
    assert!(!journal.listing().expect("listing"));
}

#[tokio::test]
async fn access_not_given_yet_is_its_own_state_and_lists_nothing() {
    let scratch = Scratch::new("drive-consent");
    let journal = DriveJournal::open(&scratch.path().join("storage_drive.sqlite")).expect("open");
    let base = WebUrl::parse("https://graph.example.test").expect("url");
    let replica =
        storage_graph::GraphReplica::new(NotGranted, &base, "", porter_sync::Clock::fixed(NOW))
            .with_root(storage_graph::Root::Drive);
    let pass = list_once(&journal, &replica).await.expect("pass");
    assert_eq!((pass.outcome, pass.pages), (Listed::NeedsConsent, 0));
    assert!(journal.is_empty().expect("empty"));
}

#[tokio::test]
async fn my_drive_is_listed_with_its_documents_as_entries_that_open_online() {
    let scratch = Scratch::new("drive-google");
    let fake = FakeGoogle::start_fixed(TOKEN).await.expect("fake google");
    fake.my_drive_put_file("Docs/a.txt", b"a");
    fake.my_drive_document("Docs/Plan", "application/vnd.google-apps.document");
    fake.my_drive_document("Survey", "application/vnd.google-apps.form");
    let journal = DriveJournal::open(&scratch.path().join("storage_drive.sqlite")).expect("open");
    let replica = google(fake.base_url(), 2, journal.folders().expect("tree"));
    let pass = list_once(&journal, &replica).await.expect("pass");
    assert_eq!(pass.outcome, Listed::Done);
    let plan = journal
        .entry_at(&path("Docs/Plan"))
        .expect("read")
        .expect("the Doc");
    let ItemKind::Online { export, .. } = &plan.kind else {
        panic!("a Doc opens online: {:?}", plan.kind)
    };
    assert_eq!(
        export.as_deref(),
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
    );
    let survey = journal
        .entry_at(&path("Survey"))
        .expect("read")
        .expect("the Form");
    assert!(
        matches!(&survey.kind, ItemKind::Online { export: None, .. }),
        "{:?}",
        survey.kind
    );
    assert_eq!(
        names(&journal, "Docs"),
        [("Plan".to_owned(), false), ("a.txt".to_owned(), false)]
    );
    let restarted =
        DriveJournal::open(&scratch.path().join("storage_drive.sqlite")).expect("open again");
    assert_eq!(restarted.len().expect("len"), journal.len().expect("len"));
}
