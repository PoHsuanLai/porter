use super::*;
use crate::testing::{MemoryListing, MemorySink, MemorySource, Refuse, Took};
use crate::words;
use porter_fake::Scratch;

struct Rig {
    dir: Scratch,
    listing: Arc<MemoryListing>,
    source: Arc<MemorySource>,
    sink: Arc<MemorySink>,
    drive: Drive,
}

fn config(dir: &Scratch) -> DriveConfig {
    DriveConfig {
        chunk: 4,
        quiet: Duration::from_millis(30),
        ..DriveConfig::new(dir.join("cache"))
    }
}

fn rig() -> Rig {
    let dir = Scratch::new("porter-drive-write");
    let listing = Arc::new(MemoryListing::default());
    let source = Arc::new(MemorySource::default());
    let sink = Arc::new(MemorySink::new(listing.clone(), source.clone(), 4));
    let drive = Drive::open_writable(config(&dir), listing.clone(), source.clone(), sink.clone())
        .expect("drive");
    Rig {
        dir,
        listing,
        source,
        sink,
        drive,
    }
}

/// `rig` with its drive closed and opened again over the same cache.
fn restarted(rig: Rig) -> Rig {
    let Rig {
        dir,
        listing,
        source,
        sink,
        drive,
    } = rig;
    drop(drive);
    let drive = Drive::open_writable(config(&dir), listing.clone(), source.clone(), sink.clone())
        .expect("drive");
    Rig {
        dir,
        listing,
        source,
        sink,
        drive,
    }
}

fn remote(id: &str) -> NodeId {
    NodeId::Remote(RemoteId(id.to_owned()))
}

impl Rig {
    fn file(&self, id: &str, parent: Option<&str>, name: &str, bytes: &[u8]) {
        self.listing
            .file(id, parent, name, bytes.len() as u64, "v1");
        self.source.put(id, bytes.to_vec());
    }

    fn state(&self, id: &NodeId) -> ItemState {
        self.drive.status(id).expect("a status").state
    }

    fn names(&self, folder: &NodeId) -> Vec<String> {
        self.drive
            .children(folder)
            .expect("children")
            .into_iter()
            .map(|n| n.name)
            .collect()
    }

    /// Makes `name` in `folder` with `bytes`, written in pieces, and closes it.
    fn write_new(&self, folder: &NodeId, name: &str, bytes: &[u8]) -> NodeId {
        let id = self.drive.create(folder, name).expect("create");
        for (at, piece) in bytes.chunks(3).enumerate() {
            self.drive
                .write(&id, (at * 3) as u64, piece)
                .expect("write");
        }
        self.drive.release_write(&id);
        id
    }

    /// Waits for `n` changes to be sent, and checks no more follow.
    async fn sent(&self, n: usize) {
        eventually(&format!("{n} changes sent"), || self.sink.took().len() >= n).await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(self.sink.took().len(), n, "{:?}", self.sink.took());
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
async fn a_new_file_is_sent_once_after_it_is_closed_and_keeps_its_inode_and_path() {
    let rig = rig();
    let id = rig.write_new(&NodeId::Top, "notes.txt", b"one two three");
    let ino = rig.drive.ino(&id);
    assert_eq!(rig.state(&id), ItemState::Waiting);
    rig.sent(1).await;
    assert_eq!(
        rig.sink.took(),
        [Took::New(None, "notes.txt".into(), 13)],
        "many writes, one upload"
    );
    let now = rig.drive.canonical(&id);
    assert!(matches!(now, NodeId::Remote(_)), "the drive's id: {now:?}");
    assert_eq!(rig.drive.ino(&now), ino);
    assert_eq!(rig.drive.ino(&id), ino);
    assert_eq!(rig.drive.by_ino(ino), Some(now.clone()));
    assert_eq!(
        rig.drive.path_of(&id).expect("path"),
        Some("notes.txt".into())
    );
    eventually("downloaded", || rig.state(&now) == ItemState::Downloaded).await;
    assert_eq!(
        rig.drive.read(&id, 0, 64).await.expect("read"),
        b"one two three"
    );
    rig.drive.listing_changed();
    assert_eq!(rig.drive.unsent(), 0);
    assert_eq!(rig.names(&NodeId::Top), ["notes.txt"], "shown once");
    assert_eq!(rig.source.fetched(), [], "its bytes were here all along");
}

#[tokio::test]
async fn a_file_made_and_removed_before_it_was_sent_is_never_sent() {
    let rig = rig();
    let id = rig.drive.create(&NodeId::Top, "scratch").expect("create");
    rig.drive.write(&id, 0, b"temp").expect("write");
    rig.drive.remove(&id).expect("remove");
    rig.drive.release_write(&id);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(rig.sink.took(), []);
    assert!(rig.names(&NodeId::Top).is_empty());
    assert_eq!(rig.drive.local_bytes(), 0);
}

#[tokio::test]
async fn saving_by_rename_over_a_file_gives_the_same_item_a_new_version() {
    let rig = rig();
    rig.file("doc", None, "report.txt", b"old");
    // Held, so the temp file is not sent before it is renamed whatever the timing.
    rig.drive.set_paused(true);
    let tmp = rig.write_new(&NodeId::Top, ".goutputstream-ABC", b"new words");
    rig.drive
        .rename(&tmp, &NodeId::Top, "report.txt")
        .expect("rename");
    rig.drive.set_paused(false);
    assert_eq!(rig.drive.canonical(&tmp), remote("doc"));
    assert_eq!(
        rig.drive.read(&remote("doc"), 0, 64).await.expect("read"),
        b"new words"
    );
    rig.sent(1).await;
    assert_eq!(rig.sink.took(), [Took::Replace("doc".into(), 9)]);
    assert_eq!(rig.names(&NodeId::Top), ["report.txt"]);
}

#[tokio::test]
async fn a_folder_is_sent_before_what_is_in_it_and_moves_keep_ids() {
    let rig = rig();
    rig.listing.folder("old", None, "Old");
    rig.listing.folder("dest", None, "Dest");
    let work = rig.drive.make_folder(&NodeId::Top, "Work").expect("mkdir");
    let inner = rig.write_new(&work, "a.txt", b"abc");
    rig.drive
        .rename(&remote("old"), &remote("dest"), "Renamed")
        .expect("move");
    rig.sent(3).await;
    let took = rig.sink.took();
    assert_eq!(took[0], Took::Folder(None, "Work".into()));
    let NodeId::Remote(work_id) = rig.drive.canonical(&work) else {
        panic!("the folder has the drive's id");
    };
    assert!(took.contains(&Took::New(Some(work_id.0.clone()), "a.txt".into(), 3)));
    assert!(took.contains(&Took::Relocate(
        "old".into(),
        Some("dest".into()),
        "Renamed".into()
    )));
    assert_eq!(
        rig.drive.path_of(&inner).expect("path"),
        Some("Work/a.txt".into())
    );
    assert_eq!(
        rig.drive.path_of(&remote("old")).expect("path"),
        Some("Dest/Renamed".into())
    );
}

#[tokio::test]
async fn removals_go_to_the_recycle_bin_what_was_in_a_folder_first() {
    let rig = rig();
    rig.listing.folder("d", None, "Dir");
    rig.file("x", Some("d"), "x.txt", b"x");
    assert_eq!(
        rig.drive.remove_folder(&remote("d")),
        Err(WriteError::NotEmpty)
    );
    rig.drive.remove(&remote("x")).expect("unlink");
    rig.drive.remove_folder(&remote("d")).expect("rmdir");
    assert!(rig.names(&NodeId::Top).is_empty());
    rig.sent(2).await;
    assert_eq!(
        rig.sink.took(),
        [Took::Trash("x".into()), Took::Trash("d".into())]
    );
    assert_eq!(rig.sink.bin(), ["x.txt", "Dir"]);
    rig.listing.file("other", None, "other", 0, "v1");
    rig.drive.listing_changed();
    assert_eq!(rig.drive.unsent(), 0);
}

#[tokio::test]
async fn offline_or_paused_a_change_waits_and_is_sent_after() {
    let rig = rig();
    rig.sink.refuse(Some(Refuse::Offline));
    let id = rig.write_new(&NodeId::Top, "a.txt", b"abc");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.state(&id), ItemState::Waiting);
    rig.drive.set_paused(true);
    rig.sink.refuse(None);
    rig.drive.set_offline(true);
    rig.drive.set_offline(false);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.sink.took(), [], "held while paused");
    assert_eq!(rig.state(&id), ItemState::Waiting);
    rig.drive.set_paused(false);
    rig.sent(1).await;
}

#[tokio::test]
async fn the_queue_survives_a_restart() {
    let rig = rig();
    rig.drive.set_paused(true);
    let id = rig.write_new(&NodeId::Top, "kept.txt", b"keep me");
    let ino = rig.drive.ino(&id);
    let rig = restarted(rig);
    assert_eq!(rig.drive.ino(&id), ino);
    assert_eq!(rig.drive.read(&id, 0, 64).await.expect("read"), b"keep me");
    rig.sent(1).await;
}

#[tokio::test]
async fn a_large_file_goes_in_chunks_and_resumes_after_a_restart() {
    let rig = rig();
    let bytes: Vec<u8> = (0..40u8).collect();
    rig.sink.cut_after(3);
    let id = rig.write_new(&NodeId::Top, "big.bin", &bytes);
    eventually("cut off", || rig.sink.cuts() == 1).await;
    let index = std::fs::read_to_string(rig.dir.join("cache").join("index.json")).expect("index");
    assert!(
        index.contains("mem://session"),
        "its session is kept: {index}"
    );
    let rig = restarted(rig);
    rig.sent(1).await;
    assert_eq!(rig.sink.largest_chunk(), 4, "never the whole file at once");
    assert_eq!(rig.sink.read_bytes(), 40, "resumed where it was cut off");
    let sent = rig.drive.canonical(&id);
    assert!(matches!(sent, NodeId::Remote(_)));
    assert_eq!(rig.drive.read(&sent, 0, 64).await.expect("read"), bytes);
}

#[tokio::test]
async fn a_refusal_keeps_the_copy_with_its_words_and_is_tried_again() {
    let rig = rig();
    rig.sink.refuse(Some(Refuse::Quota));
    let id = rig.write_new(&NodeId::Top, "big.txt", b"abc");
    eventually("refused", || {
        rig.drive
            .status(&id)
            .is_some_and(|s| s.state == ItemState::Error)
    })
    .await;
    let status = rig.drive.status(&id).expect("status");
    assert_eq!(status.words.as_deref(), Some(words::NO_ROOM));
    assert_eq!(rig.drive.read(&id, 0, 8).await.expect("kept"), b"abc");
    rig.sink.refuse(Some(Refuse::Forbidden));
    rig.drive.retry();
    eventually("refused again", || {
        rig.drive
            .status(&id)
            .is_some_and(|s| s.words.as_deref() == Some(words::MAY_NOT_WRITE))
    })
    .await;
    rig.sink.refuse(None);
    rig.listing.folder("f", None, "Other");
    rig.drive.listing_changed();
    rig.sent(1).await;
}

#[tokio::test]
async fn a_name_the_drive_refuses_is_refused_when_made_and_lock_files_stay_here() {
    let rig = rig();
    rig.sink
        .names(|name| !name.contains(':') && !name.starts_with("~$"));
    assert_eq!(
        rig.drive.create(&NodeId::Top, "a:b.txt"),
        Err(WriteError::BadName)
    );
    assert_eq!(
        rig.drive.make_folder(&NodeId::Top, "c:d"),
        Err(WriteError::BadName)
    );
    let lock = rig.write_new(&NodeId::Top, "~$Plan.docx", b"me");
    let other = rig.write_new(&NodeId::Top, ".~lock.Plan.odt#", b"me");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(rig.sink.took(), [], "lock files are never sent");
    assert_eq!(rig.state(&lock), ItemState::Downloaded);
    assert_eq!(rig.state(&other), ItemState::Downloaded);
}

#[tokio::test]
async fn eviction_never_takes_what_was_changed_here() {
    let rig = rig();
    rig.file("r", None, "r.txt", b"drive bytes");
    rig.drive.set_paused(true);
    rig.drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    rig.drive.write(&remote("r"), 0, b"DRIVE").expect("write");
    rig.drive.release_write(&remote("r"));
    rig.drive.set_limit(CacheLimit::Bytes(0));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.drive.read(&remote("r"), 0, 64).await.expect("read"),
        b"DRIVE bytes"
    );
    assert_eq!(rig.drive.remove_download(&remote("r")), Err(words::WAITING));
    assert_eq!(rig.state(&remote("r")), ItemState::Waiting);
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(rig.sink.took(), [Took::Replace("r".into(), 11)]);
}

#[tokio::test]
async fn opened_and_closed_with_nothing_written_is_left_as_the_drive_has_it() {
    let rig = rig();
    rig.file("r", None, "r.txt", b"same");
    rig.drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    rig.drive.release_write(&remote("r"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.sink.took(), []);
    assert_eq!(rig.drive.unsent(), 0);
}

#[tokio::test]
async fn a_read_only_drive_refuses_every_change() {
    let dir = Scratch::new("porter-drive-ro");
    let listing = Arc::new(MemoryListing::default());
    let source = Arc::new(MemorySource::default());
    let drive = Drive::open(config(&dir), listing, source).expect("drive");
    assert_eq!(drive.create(&NodeId::Top, "a"), Err(WriteError::ReadOnly));
    assert_eq!(
        drive.make_folder(&NodeId::Top, "a"),
        Err(WriteError::ReadOnly)
    );
}
