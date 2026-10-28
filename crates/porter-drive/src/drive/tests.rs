use super::*;
use crate::Reason;
use crate::testing::{MemoryListing, MemorySource};
use porter_sync::ItemKind;
use std::time::Duration;
use test_kit::Scratch;

struct Rig {
    dir: Scratch,
    listing: Arc<MemoryListing>,
    source: Arc<MemorySource>,
    drive: Drive,
}

fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| seed.wrapping_add(i as u8)).collect()
}

fn config(dir: &Scratch, chunk: u64) -> DriveConfig {
    DriveConfig {
        chunk,
        ..DriveConfig::new(dir.join("cache"))
    }
}

async fn rig(chunk: u64) -> Rig {
    let dir = Scratch::new("porter-drive");
    let listing = Arc::new(MemoryListing::default());
    let source = Arc::new(MemorySource::default());
    let drive = Drive::open(config(&dir, chunk), listing.clone(), source.clone())
        .await
        .expect("drive");
    Rig {
        dir,
        listing,
        source,
        drive,
    }
}

fn remote(id: &str) -> NodeId {
    NodeId::Remote(RemoteId(id.to_owned()))
}

impl Rig {
    fn file(&self, id: &str, parent: Option<&str>, name: &str, size: usize) {
        self.listing.file(id, parent, name, size as u64, "v1");
        self.source.put(id, bytes(size, id.as_bytes()[0]));
    }

    fn state(&self, id: &str) -> ItemState {
        self.drive.status(&remote(id)).expect("a status").state
    }

    fn names(&self, folder: &NodeId) -> Vec<String> {
        let mut names: Vec<String> = self
            .drive
            .children(folder)
            .expect("children")
            .into_iter()
            .map(|n| n.name)
            .collect();
        names.sort();
        names
    }
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = test_kit::Deadline::generous();
    while !deadline.passed() {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    deadline.fail(what);
}

#[tokio::test]
async fn names_follow_both_rules_and_are_the_same_after_a_restart() {
    let rig = rig(1 << 20).await;
    rig.listing.folder("f", None, "Work");
    rig.file("b2", Some("f"), "Plan.pdf", 3);
    rig.file("a1", Some("f"), "Plan.pdf", 3);
    rig.file("c3", Some("f"), "Plan 2.pdf", 3);
    rig.file("d4", Some("f"), "Q1/Q2.txt", 3);
    let folder = remote("f");
    let names = rig.names(&folder);
    assert_eq!(
        names,
        ["Plan 2.pdf", "Plan 3.pdf", "Plan.pdf", "Q1／Q2.txt"]
    );
    let oldest = rig
        .drive
        .lookup(&folder, "Plan.pdf")
        .expect("lookup")
        .expect("there");
    assert_eq!(oldest.id, remote("a1"), "the oldest by id keeps the name");
    let ino = rig.drive.ino(&remote("b2"));
    drop(rig.drive);

    let again = Drive::open(
        config(&rig.dir, 1 << 20),
        rig.listing.clone(),
        rig.source.clone(),
    )
    .await
    .expect("drive");
    let mut names_again: Vec<String> = again
        .children(&folder)
        .expect("children")
        .into_iter()
        .map(|n| n.name)
        .collect();
    names_again.sort();
    assert_eq!(names_again, names);
    assert_eq!(again.ino(&remote("b2")), ino, "inode numbers are stable");
    let path = again.path_of(&remote("b2")).expect("path").expect("there");
    assert_eq!(path, Path::new("Work/Plan 3.pdf"));
    assert_eq!(
        again.resolve(&path).expect("resolve").map(|n| n.id),
        Some(remote("b2"))
    );
}

#[tokio::test]
async fn opening_a_cloud_file_fetches_it_in_chunks_and_the_fraction_moves() {
    let rig = rig(4).await;
    rig.file("v", None, "Video.mp4", 16);
    assert_eq!(rig.state("v"), ItemState::Cloud);
    rig.source.hold();
    rig.drive.open_file(&remote("v")).expect("open");
    assert_eq!(rig.state("v"), ItemState::Downloading);
    rig.source.allow(1);
    eventually("a quarter is here", || {
        rig.drive.status(&remote("v")).and_then(|s| s.fraction) == Some(0.25)
    })
    .await;
    rig.source.allow(3);
    eventually("it is all here", || rig.state("v") == ItemState::Downloaded).await;
    let spans: Vec<(u64, u64)> = rig
        .source
        .fetched()
        .into_iter()
        .map(|(_, start, len)| (start, len))
        .collect();
    assert_eq!(spans, [(0, 4), (4, 4), (8, 4), (12, 4)]);
    let read = rig.drive.read(&remote("v"), 0, 100).await.expect("read");
    assert_eq!(read, bytes(16, b'v'));
    assert_eq!(rig.drive.local_bytes(), 16);
    rig.drive.release(&remote("v"));
}

#[tokio::test]
async fn a_read_in_the_middle_waits_for_its_own_chunk_first() {
    let rig = rig(4).await;
    rig.file("v", None, "Video.mp4", 32);
    rig.source.hold();
    rig.drive.open_file(&remote("v")).expect("open");
    // The first chunk is asked before the read comes; the read's is next, before the rest.
    eventually("the first fetch waits", || rig.source.waiting() == 1).await;
    let drive = rig.drive.clone();
    let read = tokio::spawn(async move { drive.read(&remote("v"), 25, 3).await });
    tokio::task::yield_now().await;
    rig.source.allow(1);
    eventually("the second fetch waits", || {
        rig.source.waiting() == 1 && rig.source.fetched().len() == 1
    })
    .await;
    rig.source.allow(1);
    let got = read.await.expect("task").expect("read");
    assert_eq!(got, bytes(32, b'v')[25..28].to_vec());
    let starts: Vec<u64> = rig.source.fetched().iter().map(|f| f.1).collect();
    assert_eq!(starts, [0, 24], "the chunk the read needed came second");
    assert_eq!(rig.state("v"), ItemState::Downloading);
    rig.source.release();
    eventually("the rest comes", || rig.state("v") == ItemState::Downloaded).await;
}

#[tokio::test]
async fn offline_a_read_fails_and_the_item_says_it_downloads_when_connected() {
    let rig = rig(4).await;
    rig.file("d", None, "Doc.txt", 10);
    rig.source.set_offline(true);
    rig.drive.open_file(&remote("d")).expect("open");
    let read = rig.drive.read(&remote("d"), 0, 10).await;
    assert_eq!(read, Err(ReadError::Offline));
    let status = rig.drive.status(&remote("d")).expect("status");
    assert_eq!(status.state, ItemState::Error);
    assert_eq!(status.reason, Some(Reason::Offline));
    assert_eq!(rig.drive.download(&remote("d")), Err(Reason::Offline));
    rig.source.set_offline(false);
    rig.drive.set_offline(false);
    let read = rig
        .drive
        .read(&remote("d"), 0, 10)
        .await
        .expect("connected");
    assert_eq!(read.len(), 10);
}

#[tokio::test]
async fn keeping_a_folder_fetches_what_is_under_it_and_what_is_added_later() {
    let rig = rig(4).await;
    rig.listing.folder("p", None, "Pictures");
    rig.listing.folder("q", Some("p"), "2026");
    rig.file("a", Some("p"), "a.jpg", 9);
    rig.file("b", Some("q"), "b.jpg", 5);
    rig.drive.keep(&remote("p"), true).expect("keep");
    eventually("both are here", || {
        rig.state("a") == ItemState::Downloaded && rig.state("b") == ItemState::Downloaded
    })
    .await;
    assert_eq!(rig.state("p"), ItemState::Downloaded);
    rig.file("c", Some("q"), "c.jpg", 6);
    assert!(rig.drive.status(&remote("c")).expect("status").kept);
    rig.drive.listing_changed();
    eventually("the new one is fetched", || {
        rig.state("c") == ItemState::Downloaded
    })
    .await;

    assert_eq!(
        rig.drive.remove_download(&remote("a")),
        Err(Reason::KeptWithFolder)
    );
    assert_eq!(
        rig.drive.keep(&remote("b"), false),
        Err(Reason::KeptWithFolder)
    );
    rig.drive.remove_download(&remote("p")).expect("remove");
    assert_eq!(rig.drive.local_bytes(), 0);
    assert_eq!(rig.state("a"), ItemState::Cloud);
    assert!(!rig.drive.status(&remote("b")).expect("status").kept);
    assert!(
        std::fs::read_dir(rig.drive.cache_dir().join("content"))
            .expect("dir")
            .next()
            .is_none(),
        "the bytes are gone from the disk"
    );
}

#[tokio::test]
async fn keeping_everything_fetches_the_whole_drive_and_what_is_added_and_never_evicts_it() {
    let rig = rig(4).await;
    rig.listing.folder("p", None, "Pictures");
    rig.file("a", Some("p"), "a.jpg", 9);
    rig.file("b", None, "b.txt", 5);
    rig.drive.set_limit(CacheLimit::Bytes(1));
    rig.drive.set_keep_all(true);
    assert!(rig.drive.keeps_all());
    eventually("the whole drive is here", || {
        rig.state("a") == ItemState::Downloaded && rig.state("b") == ItemState::Downloaded
    })
    .await;
    for id in ["p", "a", "b"] {
        assert!(rig.drive.status(&remote(id)).expect("status").kept, "{id}");
    }
    // What the listing adds, at any depth, is fetched too.
    rig.listing.folder("q", Some("p"), "2026");
    rig.file("c", Some("q"), "c.jpg", 6);
    rig.drive.listing_changed();
    eventually("the new one is fetched", || {
        rig.state("c") == ItemState::Downloaded
    })
    .await;
    // Past a limit of one byte, nothing goes: kept bytes are not counted against it.
    rig.drive.set_limit(CacheLimit::Bytes(1));
    for id in ["a", "b", "c"] {
        assert_eq!(rig.state(id), ItemState::Downloaded, "{id}");
    }
    assert_eq!(
        rig.drive.remove_download(&remote("a")),
        Err(Reason::KeptWithDrive)
    );
    assert_eq!(
        rig.drive.keep(&remote("p"), false),
        Err(Reason::KeptWithDrive)
    );
    assert!(!rig.drive.short_of_room());
}

#[tokio::test]
async fn keeping_everything_off_leaves_the_files_here_but_evictable_and_keeps_what_was_kept() {
    let rig = rig(4).await;
    for id in ["a", "b", "k"] {
        rig.file(id, None, &format!("{id}.bin"), 8);
    }
    rig.drive.keep(&remote("k"), true).expect("keep");
    rig.drive.set_keep_all(true);
    eventually("all here", || {
        ["a", "b", "k"]
            .iter()
            .all(|id| rig.state(id) == ItemState::Downloaded)
    })
    .await;
    rig.drive.set_keep_all(false);
    assert!(!rig.drive.keeps_all());
    for id in ["a", "b", "k"] {
        assert_eq!(
            rig.state(id),
            ItemState::Downloaded,
            "nothing removed: {id}"
        );
    }
    assert!(!rig.drive.status(&remote("a")).expect("status").kept);
    assert!(
        rig.drive.status(&remote("k")).expect("status").kept,
        "kept one by one"
    );
    // Evictable again: past the limit what is not kept goes, what was kept stays.
    rig.drive.set_limit(CacheLimit::Bytes(1));
    assert_eq!(rig.state("a"), ItemState::Cloud);
    assert_eq!(rig.state("b"), ItemState::Cloud);
    assert_eq!(rig.state("k"), ItemState::Downloaded);
}

#[tokio::test]
async fn keeping_everything_past_the_free_space_fetches_nothing_and_removes_nothing() {
    let dir = Scratch::new("porter-drive-room");
    let listing = Arc::new(MemoryListing::default());
    let source = Arc::new(MemorySource::default());
    let drive = Drive::open(
        DriveConfig {
            chunk: 4,
            free_space: FreeSpace::Bytes(10),
            ..DriveConfig::new(dir.join("cache"))
        },
        listing.clone(),
        source.clone(),
    )
    .await
    .expect("drive");
    let rig = Rig {
        dir,
        listing,
        source,
        drive,
    };
    rig.file("a", None, "a.bin", 8);
    rig.drive.download(&remote("a")).expect("download");
    eventually("here", || rig.state("a") == ItemState::Downloaded).await;
    rig.file("b", None, "b.bin", 8);
    rig.file("c", None, "c.bin", 8);
    let fetched = rig.source.fetched().len();
    let mut room = rig.drive.watch_room();
    rig.drive.set_keep_all(true);
    assert!(rig.drive.short_of_room(), "16 bytes to come, 10 free");
    assert!(room.has_changed().expect("open"), "a watcher hears it");
    assert!(*room.borrow_and_update());
    rig.drive.listing_changed();
    assert!(
        !room.has_changed().expect("open"),
        "a look that changes nothing is quiet"
    );
    assert_eq!(rig.source.fetched().len(), fetched, "nothing started");
    assert_eq!(rig.state("b"), ItemState::Cloud);
    assert_eq!(rig.state("a"), ItemState::Downloaded, "nothing removed");
    assert!(rig.drive.status(&remote("b")).expect("status").kept);
    // With less to fetch it fits on the next look.
    rig.listing.remove("c");
    rig.drive.listing_changed();
    assert!(!rig.drive.short_of_room());
    assert!(room.has_changed().expect("open"), "and hears it clear");
    assert!(!*room.borrow_and_update());
    eventually("it fits now", || rig.state("b") == ItemState::Downloaded).await;
    rig.drive.set_keep_all(false);
    assert!(!rig.drive.short_of_room(), "only while kept whole");
}

#[tokio::test]
async fn a_free_space_that_is_not_known_is_never_short_and_everything_downloads() {
    let dir = Scratch::new("porter-drive-unknown-room");
    let listing = Arc::new(MemoryListing::default());
    let source = Arc::new(MemorySource::default());
    let drive = Drive::open(
        DriveConfig {
            chunk: 4,
            free_space: FreeSpace::Unknown,
            ..DriveConfig::new(dir.join("cache"))
        },
        listing.clone(),
        source.clone(),
    )
    .await
    .expect("drive");
    let rig = Rig {
        dir,
        listing,
        source,
        drive,
    };
    rig.file("a", None, "a.bin", 8);
    rig.file("b", None, "b.bin", 8);
    rig.drive.set_keep_all(true);
    assert!(!rig.drive.short_of_room(), "nothing measured, nothing said");
    eventually("everything downloads", || {
        rig.state("a") == ItemState::Downloaded && rig.state("b") == ItemState::Downloaded
    })
    .await;
}

#[tokio::test]
async fn a_new_version_makes_the_copy_stale_and_a_kept_one_is_fetched_again() {
    let rig = rig(4).await;
    rig.file("k", None, "kept.txt", 4);
    rig.file("o", None, "other.txt", 4);
    rig.drive.keep(&remote("k"), true).expect("keep");
    rig.drive.download(&remote("o")).expect("download");
    eventually("both here", || {
        rig.state("k") == ItemState::Downloaded && rig.state("o") == ItemState::Downloaded
    })
    .await;
    rig.listing.file("k", None, "kept.txt", 4, "v2");
    rig.listing.file("o", None, "other.txt", 4, "v2");
    rig.source.put("k", b"new!".to_vec());
    assert_eq!(rig.state("o"), ItemState::Cloud, "an old version is stale");
    rig.drive.listing_changed();
    eventually("the kept one is fetched again", || {
        rig.state("k") == ItemState::Downloaded
    })
    .await;
    assert_eq!(rig.state("o"), ItemState::Cloud);
    let read = rig.drive.read(&remote("k"), 0, 4).await.expect("read");
    assert_eq!(read, b"new!");
}

#[tokio::test]
async fn eviction_keeps_the_cache_under_its_limit_and_spares_kept_and_open_files() {
    let rig = rig(1 << 20).await;
    for id in ["x", "y", "z", "w"] {
        rig.file(id, None, &format!("{id}.bin"), 8);
    }
    // The limit counts what is not kept: 16 of it.
    rig.drive.set_limit(CacheLimit::Bytes(16));
    rig.drive.keep(&remote("z"), true).expect("keep");
    eventually("kept is here", || rig.state("z") == ItemState::Downloaded).await;
    for id in ["x", "w", "y"] {
        rig.drive.open_file(&remote(id)).expect("open");
        rig.drive.read(&remote(id), 0, 8).await.expect("read");
        eventually("it is here", || rig.state(id) == ItemState::Downloaded).await;
        if id != "w" {
            rig.drive.release(&remote(id));
        }
    }
    // 24 bytes not kept: the oldest opened that is neither kept nor open goes.
    eventually("the oldest goes", || rig.state("x") == ItemState::Cloud).await;
    assert_eq!(rig.state("z"), ItemState::Downloaded, "kept");
    assert_eq!(rig.state("w"), ItemState::Downloaded, "open");
    assert_eq!(rig.state("y"), ItemState::Downloaded);
    assert!(rig.drive.local_bytes() <= 24);
    rig.drive.set_limit(CacheLimit::Bytes(1));
    assert_eq!(rig.state("y"), ItemState::Cloud);
    assert_eq!(rig.state("w"), ItemState::Downloaded, "still open");
    rig.drive.release(&remote("w"));
    eventually("closed, it goes", || rig.state("w") == ItemState::Cloud).await;
    assert_eq!(rig.state("z"), ItemState::Downloaded, "never a kept one");
}

#[tokio::test]
async fn an_online_document_reads_as_its_link_and_is_never_downloaded() {
    let rig = rig(4).await;
    rig.listing.online(
        "doc1",
        None,
        "Plan",
        "https://docs.example.test/document/d/doc1",
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    );
    let node = rig
        .drive
        .lookup(&NodeId::Top, "Plan.gdoc")
        .expect("lookup")
        .expect("there");
    let read = rig.drive.read(&node.id, 0, 4096).await.expect("read");
    assert_eq!(read.len() as u64, node.size);
    let link: serde_json::Value = serde_json::from_slice(&read).expect("json");
    assert_eq!(link["url"], "https://docs.example.test/document/d/doc1");
    assert_eq!(link["doc_id"], "doc1");
    assert_eq!(rig.drive.download(&node.id), Err(Reason::OnlineOnly));
    assert_eq!(rig.drive.keep(&node.id, true), Err(Reason::OnlineOnly));
    assert_eq!(rig.drive.remove_download(&node.id), Err(Reason::OnlineOnly));
    assert!(rig.source.fetched().is_empty());
    assert_eq!(rig.drive.local_bytes(), 0);
}

#[tokio::test]
async fn what_is_here_and_kept_is_remembered_across_a_restart() {
    let rig = rig(4).await;
    rig.file("k", None, "kept.txt", 6);
    rig.drive.keep(&remote("k"), true).expect("keep");
    eventually("here", || rig.state("k") == ItemState::Downloaded).await;
    rig.drive.flush().expect("flush");
    drop(rig.drive);
    let again = Drive::open(config(&rig.dir, 4), rig.listing.clone(), rig.source.clone())
        .await
        .expect("drive");
    let status = again.status(&remote("k")).expect("status");
    assert_eq!(status.state, ItemState::Downloaded);
    assert!(status.kept);
    assert_eq!(again.local_bytes(), 6);
}

const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

#[tokio::test]
async fn a_copy_is_saved_next_to_its_document_named_as_it_is_and_waits_to_upload() {
    let rig = rig(4).await;
    rig.listing.folder("f", None, "Work");
    let url = "https://docs.example.test/document/d/doc1";
    rig.listing
        .online("doc1", Some("f"), "Plan", url, Some(DOCX));
    rig.file("old", Some("f"), "Plan.docx", 3);
    rig.source.put_export("doc1", b"docx bytes".to_vec());
    let plan = rig
        .drive
        .resolve("Work/Plan.gdoc".as_ref())
        .expect("resolve")
        .expect("there");

    let copy = rig.drive.save_copy(&plan.id).await.expect("copy");
    let node = rig.drive.node(&copy).expect("node").expect("there");
    assert_eq!(node.name, "Plan 2.docx", "Plan.docx is taken");
    assert_eq!(
        rig.drive.path_of(&copy).expect("path"),
        Some("Work/Plan 2.docx".into())
    );
    assert_eq!(
        rig.drive.read(&copy, 0, 64).await.expect("read"),
        b"docx bytes"
    );
    let status = rig.drive.status(&copy).expect("status");
    assert_eq!(status.state, ItemState::Waiting);
    assert_eq!(
        rig.drive.remove_download(&copy),
        Err(Reason::Waiting),
        "nothing made here is lost"
    );
    let again = rig.drive.save_copy(&plan.id).await.expect("again");
    assert_eq!(
        rig.drive.node(&again).expect("node").expect("there").name,
        "Plan 3.docx"
    );

    // The same names after a restart.
    drop(rig.drive);
    let reopened = Drive::open(config(&rig.dir, 4), rig.listing.clone(), rig.source.clone())
        .await
        .expect("drive");
    let names: Vec<String> = reopened
        .children(&remote("f"))
        .expect("children")
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert_eq!(
        names,
        ["Plan.gdoc", "Plan.docx", "Plan 2.docx", "Plan 3.docx"]
    );
}

#[tokio::test]
async fn save_a_copy_refuses_what_has_no_file_and_says_why() {
    let rig = rig(4).await;
    rig.file("bin", None, "Notes.txt", 3);
    let form = "https://docs.example.test/forms/d/form1";
    rig.listing.online("form1", None, "Survey", form, None);
    let big = "https://docs.example.test/document/d/big";
    rig.listing.online("big", None, "Book", big, Some(DOCX));
    rig.source.refuse_export("big", crate::CopyError::TooLarge);
    let doc = "https://docs.example.test/document/d/doc";
    rig.listing.online("doc", None, "Plan", doc, Some(DOCX));
    rig.source.put_export("doc", b"x".to_vec());

    let copy = |id: &str| {
        let drive = rig.drive.clone();
        let id = remote(id);
        async move { drive.save_copy(&id).await }
    };
    assert_eq!(copy("bin").await, Err(Reason::NotADocument));
    assert_eq!(copy("form1").await, Err(Reason::NoFileFormat));
    assert_eq!(copy("big").await, Err(Reason::TooLarge));
    rig.source.set_offline(true);
    assert_eq!(copy("doc").await, Err(Reason::CopyFailed));
    assert!(
        rig.drive
            .children(&NodeId::Top)
            .expect("children")
            .iter()
            .all(|n| !matches!(n.id, NodeId::Local(_))),
        "no copy was made"
    );
}

async fn reopen(dir: &Scratch, listing: &Arc<MemoryListing>, source: &Arc<MemorySource>) -> Drive {
    Drive::open(config(dir, 4), listing.clone(), source.clone())
        .await
        .expect("drive")
}

#[tokio::test]
async fn a_second_drive_over_one_cache_waits_for_the_first_to_go() {
    let rig = rig(4).await;
    rig.file("k", None, "kept.txt", 4);
    rig.drive.keep(&remote("k"), true).expect("keep");
    eventually("here", || rig.state("k") == ItemState::Downloaded).await;
    let soon = DriveConfig {
        lock_wait: Duration::from_millis(50),
        ..config(&rig.dir, 4)
    };
    let refused = Drive::open(soon, rig.listing.clone(), rig.source.clone()).await;
    assert!(
        matches!(refused, Err(DriveError::InUse)),
        "the first is still open: {refused:?}"
    );

    // The first goes while the second waits: the second reads what the first wrote last, and
    // nothing the first does after can write over it.
    let first = rig.drive.clone();
    let gone = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        first.keep(&remote("k"), false).expect("unkeep");
        drop(first);
    });
    let Rig {
        dir,
        listing,
        source,
        drive,
    } = rig;
    drop(drive);
    let second = Drive::open(config(&dir, 4), listing.clone(), source.clone())
        .await
        .expect("opens");
    gone.join().expect("thread");
    assert!(!second.status(&remote("k")).expect("status").kept);
    second.keep(&remote("k"), true).expect("keep");
    second.flush().expect("flush");
    drop(second);
    let third = reopen(&dir, &listing, &source).await;
    assert!(third.status(&remote("k")).expect("status").kept);
}

#[tokio::test]
async fn the_index_claims_only_bytes_synced_and_its_old_copy_claims_none() {
    let rig = rig(4).await;
    rig.file("k", None, "kept.txt", 6);
    rig.drive.keep(&remote("k"), true).expect("keep");
    eventually("here", || rig.state("k") == ItemState::Downloaded).await;
    rig.drive.flush().expect("flush");
    let content = rig.drive.inner.content(&RemoteId("k".into()), "v1");
    assert!(
        held(&rig.drive.inner.synced).contains(&content),
        "the content file was synced before the index noted it"
    );

    // One more write: the index before it is the copy beside it. The index is then lost.
    rig.file("o", None, "other.txt", 2);
    rig.drive.download(&remote("o")).expect("download");
    eventually("here", || rig.state("o") == ItemState::Downloaded).await;
    rig.drive.flush().expect("flush");
    let Rig {
        dir,
        listing,
        source,
        drive,
    } = rig;
    let index = drive.cache_dir().join("index.json");
    drop(drive);
    std::fs::write(&index, b"{ not json").expect("break the index");
    let again = reopen(&dir, &listing, &source).await;
    let status = again.status(&remote("k")).expect("status");
    assert!(status.kept, "what is kept stands");
    assert_eq!(
        status.state,
        ItemState::Cloud,
        "no ranges from the old copy"
    );
    assert_eq!(again.local_bytes(), 0);
    again.listing_changed();
    eventually("kept, it is fetched again", || {
        again.status(&remote("k")).map(|s| s.state) == Some(ItemState::Downloaded)
    })
    .await;
}

#[tokio::test]
async fn a_cancelled_transfer_says_nothing_of_the_one_after_it() {
    let rig = rig(4).await;
    rig.file("d", None, "Doc.txt", 4);
    rig.source.hold();
    rig.drive.open_file(&remote("d")).expect("open");
    eventually("the first fetch waits", || rig.source.waiting() == 1).await;
    rig.drive.remove_download(&remote("d")).expect("remove");
    rig.drive.open_file(&remote("d")).expect("open again");
    eventually("the second fetch waits", || rig.source.waiting() == 2).await;
    // The first, cancelled, fails; the second comes.
    rig.source.set_offline(true);
    rig.source.allow(1);
    eventually("the first is through", || rig.source.waiting() == 1).await;
    rig.source.set_offline(false);
    rig.source.release();
    eventually("the second is here", || {
        rig.state("d") == ItemState::Downloaded
    })
    .await;
    assert!(
        !rig.drive.offline(),
        "the cancelled one's failure is not the drive's"
    );
}

fn hashed(rig: &Rig, id: &str, size: u64, version: &str, of: &[u8]) {
    let mut entry = crate::testing::entry(
        id,
        None,
        &format!("{id}.bin"),
        ItemKind::File,
        size,
        version,
    );
    entry.hash = crate::hash::of_bytes(porter_capability::HashKind::Sha256, of)
        .map(porter_sync::ContentHash);
    rig.listing.put(entry);
}

#[tokio::test]
async fn a_download_is_checked_against_the_drives_hash_and_dropped_when_it_differs() {
    let rig = rig(4).await;
    rig.source.set_hashes(porter_capability::HashKind::Sha256);
    hashed(&rig, "g", 8, "v1", b"12345678");
    rig.source.put("g", b"12345678".to_vec());
    rig.drive.download(&remote("g")).expect("download");
    eventually("it is here", || rig.state("g") == ItemState::Downloaded).await;

    // The drive has another version than the listing says: what comes is not it.
    hashed(&rig, "h", 8, "v1", b"abcdefgh");
    rig.source.put("h", b"abcdXXXX".to_vec());
    rig.drive.open_file(&remote("h")).expect("open");
    eventually("it is dropped", || {
        rig.state("h") == ItemState::Cloud
            && rig.drive.inner.transfer(&RemoteId("h".into())).is_none()
    })
    .await;
    assert_eq!(rig.drive.local_bytes(), 8, "only the first is here");
    rig.drive.release(&remote("h"));
}

#[tokio::test]
async fn with_no_hash_a_download_whose_version_moved_on_is_dropped() {
    let rig = rig(4).await;
    rig.file("v", None, "v.bin", 8);
    rig.source.hold();
    rig.drive.open_file(&remote("v")).expect("open");
    eventually("the first fetch waits", || rig.source.waiting() == 1).await;
    rig.listing.file("v", None, "v.bin", 8, "v2");
    rig.source.release();
    eventually("the bytes of v1 are dropped", || {
        rig.drive.inner.transfer(&RemoteId("v".into())).is_none() && rig.drive.local_bytes() == 0
    })
    .await;
    rig.drive.release(&remote("v"));
}

#[tokio::test]
async fn a_waiting_read_joins_the_newer_version_instead_of_fighting_it() {
    let rig = rig(4).await;
    rig.file("n", None, "n.bin", 8);
    rig.source.hold();
    rig.drive.open_file(&remote("n")).expect("open");
    eventually("the first fetch waits", || rig.source.waiting() == 1).await;
    let drive = rig.drive.clone();
    let read = tokio::spawn(async move { drive.read(&remote("n"), 4, 4).await });
    tokio::task::yield_now().await;
    rig.listing.file("n", None, "n.bin", 8, "v2");
    rig.source.put("n", b"NEWBYTES".to_vec());
    rig.drive.open_file(&remote("n")).expect("open the new one");
    rig.source.release();
    let got = read.await.expect("task").expect("read");
    assert_eq!(got, b"YTES");
    eventually("the new version is here", || {
        rig.state("n") == ItemState::Downloaded
    })
    .await;
    rig.drive.release(&remote("n"));
    rig.drive.release(&remote("n"));
}

#[tokio::test]
async fn the_limit_counts_only_what_is_not_kept() {
    let rig = rig(1 << 20).await;
    rig.file("k", None, "k.bin", 16);
    rig.file("a", None, "a.bin", 8);
    rig.file("b", None, "b.bin", 8);
    rig.drive.set_limit(CacheLimit::Bytes(20));
    rig.drive.keep(&remote("k"), true).expect("keep");
    rig.drive.download(&remote("a")).expect("download");
    rig.drive.download(&remote("b")).expect("download");
    eventually("all here", || {
        ["k", "a", "b"]
            .iter()
            .all(|id| rig.state(id) == ItemState::Downloaded)
    })
    .await;
    rig.drive.set_limit(CacheLimit::Bytes(20));
    assert_eq!(
        rig.state("a"),
        ItemState::Downloaded,
        "16 bytes not kept, under 20"
    );
    assert_eq!(rig.state("b"), ItemState::Downloaded);
}

#[tokio::test]
async fn nothing_is_evicted_when_the_folders_above_cannot_be_read() {
    let rig = rig(1 << 20).await;
    rig.file("a", None, "a.bin", 8);
    rig.drive.download(&remote("a")).expect("download");
    eventually("here", || rig.state("a") == ItemState::Downloaded).await;
    rig.listing.set_unreadable(true);
    rig.drive.set_limit(CacheLimit::Bytes(1));
    rig.listing.set_unreadable(false);
    assert_eq!(rig.state("a"), ItemState::Downloaded, "taken as kept");
}

#[tokio::test]
async fn a_copy_is_on_the_disk_when_answered_and_one_the_index_lost_is_taken_in() {
    let rig = rig(4).await;
    let url = "https://docs.example.test/document/d/doc1";
    rig.listing.online("doc1", None, "Plan", url, Some(DOCX));
    rig.source.put_export("doc1", b"docx".to_vec());
    let copy = rig.drive.save_copy(&remote("doc1")).await.expect("copy");
    let NodeId::Local(n) = copy else {
        panic!("a local node")
    };
    let stored = std::fs::read_to_string(rig.drive.cache_dir().join("index.json")).expect("index");
    assert!(
        stored.contains("Plan.docx"),
        "written before the answer: {stored}"
    );

    // A file made here that the index never noted (a crash between the two writes).
    let lost = rig
        .drive
        .cache_dir()
        .join("local")
        .join((n + 4).to_string());
    std::fs::write(&lost, b"lost").expect("write");
    let Rig {
        dir,
        listing,
        source,
        drive,
    } = rig;
    drop(drive);
    let again = reopen(&dir, &listing, &source).await;
    let node = again
        .lookup(&NodeId::Top, &format!("Recovered File {}", n + 4))
        .expect("lookup")
        .expect("taken in");
    assert_eq!(again.read(&node.id, 0, 16).await.expect("read"), b"lost");
    let next = again.save_copy(&remote("doc1")).await.expect("copy");
    assert_eq!(next, NodeId::Local(n + 5), "its number is not given again");
}

#[tokio::test]
async fn an_empty_listing_drops_nothing() {
    let rig = rig(4).await;
    rig.file("k", None, "k.bin", 4);
    rig.drive.keep(&remote("k"), true).expect("keep");
    eventually("here", || rig.state("k") == ItemState::Downloaded).await;
    rig.listing.remove("k");
    rig.drive.listing_changed();
    rig.file("k", None, "k.bin", 4);
    let status = rig.drive.status(&remote("k")).expect("status");
    assert_eq!(status.state, ItemState::Downloaded);
    assert!(status.kept);
}

#[tokio::test]
async fn reached_again_a_file_that_failed_offline_is_in_the_cloud_again() {
    let rig = rig(4).await;
    rig.file("d", None, "Doc.txt", 10);
    rig.source.set_offline(true);
    rig.drive.open_file(&remote("d")).expect("open");
    assert_eq!(
        rig.drive.read(&remote("d"), 0, 10).await,
        Err(ReadError::Offline)
    );
    assert_eq!(rig.state("d"), ItemState::Error);
    rig.source.set_offline(false);
    rig.drive.set_offline(false);
    assert_eq!(rig.state("d"), ItemState::Cloud);
    rig.drive.release(&remote("d"));
}

#[tokio::test]
async fn a_stalled_fetch_fails_its_reader_at_its_deadline() {
    let dir = Scratch::new("porter-drive");
    let listing = Arc::new(MemoryListing::default());
    let source = Arc::new(MemorySource::default());
    let drive = Drive::open(
        DriveConfig {
            deadline: Duration::from_millis(50),
            ..config(&dir, 4)
        },
        listing.clone(),
        source.clone(),
    )
    .await
    .expect("drive");
    listing.file("s", None, "s.bin", 4, "v1");
    source.put("s", b"slow".to_vec());
    source.hold();
    drive.open_file(&remote("s")).expect("open");
    let read = tokio::time::timeout(Duration::from_secs(20), drive.read(&remote("s"), 0, 4)).await;
    assert_eq!(read, Ok(Err(ReadError::Stalled)));
    source.release();
    assert_eq!(drive.read(&remote("s"), 0, 4).await.expect("read"), b"slow");
    drive.release(&remote("s"));
}
