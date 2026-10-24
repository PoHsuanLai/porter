use super::*;
use crate::testing::{MemoryListing, MemorySource};
use crate::words;
use porter_fake::Scratch;
use std::time::Duration;

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

fn rig(chunk: u64) -> Rig {
    let dir = Scratch::new("porter-drive");
    let listing = Arc::new(MemoryListing::default());
    let source = Arc::new(MemorySource::default());
    let drive = Drive::open(config(&dir, chunk), listing.clone(), source.clone()).expect("drive");
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
    let deadline = porter_fake::Deadline::generous();
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
    let rig = rig(1 << 20);
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

    let again = Drive::open(
        config(&rig.dir, 1 << 20),
        rig.listing.clone(),
        rig.source.clone(),
    )
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
    let rig = rig(4);
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
    let rig = rig(4);
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
    let rig = rig(4);
    rig.file("d", None, "Doc.txt", 10);
    rig.source.set_offline(true);
    rig.drive.open_file(&remote("d")).expect("open");
    let read = rig.drive.read(&remote("d"), 0, 10).await;
    assert_eq!(read, Err(ReadError::Offline));
    let status = rig.drive.status(&remote("d")).expect("status");
    assert_eq!(status.state, ItemState::Error);
    assert_eq!(status.words.as_deref(), Some(words::OFFLINE));
    assert_eq!(rig.drive.download(&remote("d")), Err(words::OFFLINE));
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
    let rig = rig(4);
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
        Err(words::KEPT_WITH_FOLDER)
    );
    assert_eq!(
        rig.drive.keep(&remote("b"), false),
        Err(words::KEPT_WITH_FOLDER)
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
async fn a_new_version_makes_the_copy_stale_and_a_kept_one_is_fetched_again() {
    let rig = rig(4);
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
    let rig = rig(1 << 20);
    for id in ["x", "y", "z", "w"] {
        rig.file(id, None, &format!("{id}.bin"), 8);
    }
    rig.drive.set_limit(CacheLimit::Bytes(24));
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
    // 32 bytes: the oldest opened that is neither kept nor open goes.
    eventually("the oldest goes", || rig.state("x") == ItemState::Cloud).await;
    assert_eq!(rig.state("z"), ItemState::Downloaded, "kept");
    assert_eq!(rig.state("w"), ItemState::Downloaded, "open");
    assert_eq!(rig.state("y"), ItemState::Downloaded);
    assert!(rig.drive.local_bytes() <= 24);
    rig.drive.set_limit(CacheLimit::Bytes(1));
    assert_eq!(rig.state("y"), ItemState::Cloud);
    assert_eq!(rig.state("w"), ItemState::Downloaded, "still open");
    rig.drive.release(&remote("w"));
    assert_eq!(rig.state("w"), ItemState::Cloud);
    assert_eq!(rig.state("z"), ItemState::Downloaded, "never a kept one");
}

#[tokio::test]
async fn an_online_document_reads_as_its_link_and_is_never_downloaded() {
    let rig = rig(4);
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
    assert_eq!(rig.drive.download(&node.id), Err(words::ONLINE_ONLY));
    assert_eq!(rig.drive.keep(&node.id, true), Err(words::ONLINE_ONLY));
    assert_eq!(rig.drive.remove_download(&node.id), Err(words::ONLINE_ONLY));
    assert!(rig.source.fetched().is_empty());
    assert_eq!(rig.drive.local_bytes(), 0);
}

#[tokio::test]
async fn what_is_here_and_kept_is_remembered_across_a_restart() {
    let rig = rig(4);
    rig.file("k", None, "kept.txt", 6);
    rig.drive.keep(&remote("k"), true).expect("keep");
    eventually("here", || rig.state("k") == ItemState::Downloaded).await;
    rig.drive.flush().expect("flush");
    let again =
        Drive::open(config(&rig.dir, 4), rig.listing.clone(), rig.source.clone()).expect("drive");
    let status = again.status(&remote("k")).expect("status");
    assert_eq!(status.state, ItemState::Downloaded);
    assert!(status.kept);
    assert_eq!(again.local_bytes(), 6);
}
