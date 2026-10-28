use super::*;
use crate::Reason;
use crate::testing::{MemoryListing, MemorySink, MemorySource, Refuse, Took};
use test_kit::Scratch;

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

async fn rig() -> Rig {
    let dir = Scratch::new("porter-drive-write");
    let listing = Arc::new(MemoryListing::default());
    let source = Arc::new(MemorySource::default());
    let sink = Arc::new(MemorySink::new(listing.clone(), source.clone(), 4));
    let drive = Drive::open_writable(config(&dir), listing.clone(), source.clone(), sink.clone())
        .await
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
async fn restarted(rig: Rig) -> Rig {
    let Rig {
        dir,
        listing,
        source,
        sink,
        drive,
    } = rig;
    drop(drive);
    // The cache's lock goes with the drive's last sender task, which can still be finishing its
    // flush when the handle is dropped; opening waits for it (yielding to the runtime, so the
    // task can run), as a person reopening would.
    let drive = Drive::open_writable(
        DriveConfig {
            lock_wait: test_kit::GENEROUS,
            ..config(&dir)
        },
        listing.clone(),
        source.clone(),
        sink.clone(),
    )
    .await
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
    async fn write_new(&self, folder: &NodeId, name: &str, bytes: &[u8]) -> NodeId {
        let (id, handle) = self.drive.create(folder, name).expect("create");
        for (at, piece) in bytes.chunks(3).enumerate() {
            self.drive
                .write(handle, (at * 3) as u64, piece)
                .await
                .expect("write");
        }
        self.drive.release_write(handle);
        id
    }

    /// Opens the drive's file `id`, writes `bytes` at `at` and closes it.
    async fn write_at(&self, id: &str, at: u64, bytes: &[u8]) {
        let handle = self
            .drive
            .open_write(&remote(id), false)
            .await
            .expect("open");
        self.drive.write(handle, at, bytes).await.expect("write");
        self.drive.release_write(handle);
    }

    /// Waits for `n` changes to be sent, and checks no more follow.
    async fn sent(&self, n: usize) {
        eventually(&format!("{n} changes sent"), || self.sink.took().len() >= n).await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(self.sink.took().len(), n, "{:?}", self.sink.took());
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
async fn a_new_file_is_sent_once_after_it_is_closed_and_keeps_its_inode_and_path() {
    let rig = rig().await;
    let id = rig
        .write_new(&NodeId::Top, "notes.txt", b"one two three")
        .await;
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
    let rig = rig().await;
    let (id, handle) = rig.drive.create(&NodeId::Top, "scratch").expect("create");
    rig.drive.write(handle, 0, b"temp").await.expect("write");
    rig.drive.remove(&id).expect("remove");
    assert_eq!(
        rig.drive.read(&id, 0, 8).await.expect("still open"),
        b"temp"
    );
    rig.drive.release_write(handle);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(rig.sink.took(), []);
    assert!(rig.names(&NodeId::Top).is_empty());
    assert_eq!(rig.drive.local_bytes(), 0);
}

#[tokio::test]
async fn saving_by_rename_over_a_file_gives_the_same_item_a_new_version() {
    let rig = rig().await;
    rig.file("doc", None, "report.txt", b"old");
    // Held, so the temp file is not sent before it is renamed whatever the timing.
    rig.drive.set_paused(true);
    let tmp = rig
        .write_new(&NodeId::Top, ".goutputstream-ABC", b"new words")
        .await;
    rig.drive
        .rename(&tmp, &NodeId::Top, "report.txt")
        .await
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
    let rig = rig().await;
    rig.listing.folder("old", None, "Old");
    rig.listing.folder("dest", None, "Dest");
    let work = rig.drive.make_folder(&NodeId::Top, "Work").expect("mkdir");
    let inner = rig.write_new(&work, "a.txt", b"abc").await;
    rig.drive
        .rename(&remote("old"), &remote("dest"), "Renamed")
        .await
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
    let rig = rig().await;
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
    let rig = rig().await;
    rig.sink.refuse(Some(Refuse::Offline));
    let id = rig.write_new(&NodeId::Top, "a.txt", b"abc").await;
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
    let rig = rig().await;
    rig.drive.set_paused(true);
    let id = rig.write_new(&NodeId::Top, "kept.txt", b"keep me").await;
    let ino = rig.drive.ino(&id);
    let rig = restarted(rig).await;
    assert_eq!(rig.drive.ino(&id), ino);
    assert_eq!(rig.drive.read(&id, 0, 64).await.expect("read"), b"keep me");
    rig.sent(1).await;
}

#[tokio::test]
async fn a_large_file_goes_in_chunks_and_resumes_after_a_restart() {
    let rig = rig().await;
    let bytes: Vec<u8> = (0..40u8).collect();
    rig.sink.cut_after(3);
    let id = rig.write_new(&NodeId::Top, "big.bin", &bytes).await;
    eventually("cut off", || rig.sink.cuts() == 1).await;
    // The drive writes its index after the cut, on its own flush, so the test waits for it
    // rather than reading it the moment the cut is seen.
    let index = rig.dir.join("cache").join("index.json");
    eventually("its session is kept in the index", || {
        std::fs::read_to_string(&index).is_ok_and(|kept| kept.contains("mem://session"))
    })
    .await;
    let rig = restarted(rig).await;
    rig.sent(1).await;
    assert_eq!(rig.sink.largest_chunk(), 4, "never the whole file at once");
    assert_eq!(rig.sink.read_bytes(), 40, "resumed where it was cut off");
    let sent = rig.drive.canonical(&id);
    assert!(matches!(sent, NodeId::Remote(_)));
    assert_eq!(rig.drive.read(&sent, 0, 64).await.expect("read"), bytes);
}

#[tokio::test]
async fn a_refusal_keeps_the_copy_with_its_reason_and_is_tried_again() {
    let rig = rig().await;
    rig.sink.refuse(Some(Refuse::Quota));
    let id = rig.write_new(&NodeId::Top, "big.txt", b"abc").await;
    eventually("refused", || {
        rig.drive
            .status(&id)
            .is_some_and(|s| s.state == ItemState::Error)
    })
    .await;
    let status = rig.drive.status(&id).expect("status");
    assert_eq!(status.reason, Some(Reason::NoRoom));
    assert_eq!(rig.drive.read(&id, 0, 8).await.expect("kept"), b"abc");
    rig.sink.refuse(Some(Refuse::Forbidden));
    rig.drive.retry();
    eventually("refused again", || {
        rig.drive
            .status(&id)
            .is_some_and(|s| s.reason == Some(Reason::MayNotWrite))
    })
    .await;
    rig.sink.refuse(None);
    rig.listing.folder("f", None, "Other");
    rig.drive.listing_changed();
    rig.sent(1).await;
}

#[tokio::test]
async fn a_name_the_drive_refuses_is_refused_when_made_and_lock_files_stay_here() {
    let rig = rig().await;
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
    let lock = rig.write_new(&NodeId::Top, "~$Plan.docx", b"me").await;
    let other = rig.write_new(&NodeId::Top, ".~lock.Plan.odt#", b"me").await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(rig.sink.took(), [], "lock files are never sent");
    assert_eq!(rig.state(&lock), ItemState::Downloaded);
    assert_eq!(rig.state(&other), ItemState::Downloaded);
}

#[tokio::test]
async fn eviction_never_takes_what_was_changed_here() {
    let rig = rig().await;
    rig.file("r", None, "r.txt", b"drive bytes");
    rig.drive.set_paused(true);
    rig.write_at("r", 0, b"DRIVE").await;
    rig.drive.set_limit(CacheLimit::Bytes(0));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.drive.read(&remote("r"), 0, 64).await.expect("read"),
        b"DRIVE bytes"
    );
    assert_eq!(
        rig.drive.remove_download(&remote("r")),
        Err(Reason::Waiting)
    );
    assert_eq!(rig.state(&remote("r")), ItemState::Waiting);
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(rig.sink.took(), [Took::Replace("r".into(), 11)]);
}

#[tokio::test]
async fn opened_and_closed_with_nothing_written_is_left_as_the_drive_has_it() {
    let rig = rig().await;
    rig.file("r", None, "r.txt", b"same");
    let handle = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    rig.drive.release_write(handle);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.sink.took(), []);
    assert_eq!(rig.drive.unsent(), 0);
}

#[tokio::test]
async fn a_read_only_drive_refuses_every_change() {
    let dir = Scratch::new("porter-drive-ro");
    let listing = Arc::new(MemoryListing::default());
    let source = Arc::new(MemorySource::default());
    let drive = Drive::open(config(&dir), listing, source)
        .await
        .expect("drive");
    assert_eq!(drive.create(&NodeId::Top, "a"), Err(WriteError::ReadOnly));
    assert_eq!(
        drive.make_folder(&NodeId::Top, "a"),
        Err(WriteError::ReadOnly)
    );
}

/// Changes `id` on the drive behind the core's back: a new version with `bytes`.
fn changed_there(rig: &Rig, id: &str, name: &str, bytes: &[u8], version: &str) {
    rig.listing
        .file(id, None, name, bytes.len() as u64, version);
    rig.source.put(id, bytes.to_vec());
}

#[tokio::test]
async fn a_newer_version_on_the_drive_keeps_both_and_each_names_the_other() {
    let rig = rig().await;
    rig.file("r", None, "Plan.txt", b"old");
    rig.listing
        .file("taken", None, "Plan (this computer).txt", 1, "v1");
    rig.drive.set_paused(true);
    let handle = rig
        .drive
        .open_write(&remote("r"), true)
        .await
        .expect("open");
    rig.drive.write(handle, 0, b"mine").await.expect("write");
    rig.drive.release_write(handle);
    changed_there(&rig, "r", "Plan.txt", b"theirs", "v2");
    rig.drive.listing_changed();
    let mut names = rig.names(&NodeId::Top);
    names.sort();
    assert_eq!(
        names,
        [
            "Plan (this computer) 2.txt",
            "Plan (this computer).txt",
            "Plan.txt"
        ]
    );
    let copy = rig
        .drive
        .lookup(&NodeId::Top, "Plan (this computer) 2.txt")
        .expect("lookup")
        .expect("there")
        .id;
    let theirs = rig.drive.status(&remote("r")).expect("status");
    assert_eq!(theirs.state, ItemState::Conflict);
    assert_eq!(theirs.other, Some(copy.clone()));
    let mine = rig.drive.status(&copy).expect("status");
    assert_eq!(mine.state, ItemState::Conflict);
    assert_eq!(mine.other, Some(remote("r")));
    assert_eq!(rig.drive.read(&copy, 0, 64).await.expect("read"), b"mine");
    assert_eq!(
        rig.drive.read(&remote("r"), 0, 64).await.expect("read"),
        b"theirs"
    );
    assert_eq!(rig.drive.remove_download(&copy), Err(Reason::Conflict));
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(
        rig.sink.took(),
        [Took::New(None, "Plan (this computer) 2.txt".into(), 4)]
    );
    let sent = rig.drive.canonical(&copy);
    assert!(matches!(sent, NodeId::Remote(_)));
    assert_eq!(
        rig.drive.status(&remote("r")).expect("status").other,
        Some(sent)
    );
}

#[tokio::test]
async fn bytes_not_sent_of_a_file_the_drive_removed_are_sent_again_as_a_new_file() {
    let rig = rig().await;
    rig.listing.folder("keep", None, "Keep");
    rig.file("r", None, "Notes.txt", b"old");
    rig.drive.set_paused(true);
    let handle = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    rig.drive
        .write(handle, 3, b" and new")
        .await
        .expect("write");
    let ino = rig.drive.ino(&remote("r"));
    rig.drive.release_write(handle);
    rig.listing.remove("r");
    rig.drive.listing_changed();
    let node = rig
        .drive
        .lookup(&NodeId::Top, "Notes.txt")
        .expect("lookup")
        .expect("kept");
    assert_eq!(rig.drive.ino(&node.id), ino, "the same file");
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(rig.sink.took(), [Took::New(None, "Notes.txt".into(), 11)]);
}

#[tokio::test]
async fn a_removal_of_a_file_changed_on_the_drive_brings_the_drives_version_back() {
    let rig = rig().await;
    rig.listing.folder("keep", None, "Keep");
    rig.file("r", None, "Plan.txt", b"old");
    rig.drive.set_paused(true);
    rig.drive.remove(&remote("r")).expect("unlink");
    assert_eq!(rig.names(&NodeId::Top), ["Keep"]);
    changed_there(&rig, "r", "Plan.txt", b"newer", "v2");
    rig.drive.set_paused(false);
    // The drive refuses the stale removal, and wants the listing.
    let wanted = rig.drive.listing_wanted();
    tokio::time::timeout(Duration::from_secs(10), wanted.notified())
        .await
        .expect("a listing pass is wanted");
    rig.drive.listing_changed();
    let mut names = rig.names(&NodeId::Top);
    names.sort();
    assert_eq!(names, ["Keep", "Plan.txt"]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.sink.took(), [], "nothing was removed");
    assert_eq!(rig.drive.unsent(), 0);
}

#[tokio::test]
async fn a_rename_is_made_again_on_new_bytes_and_yields_to_a_rename_there() {
    let rig = rig().await;
    rig.file("a", None, "a.txt", b"a");
    rig.file("b", None, "b.txt", b"b");
    rig.drive.set_paused(true);
    rig.drive
        .rename(&remote("a"), &NodeId::Top, "a2.txt")
        .await
        .expect("rename");
    rig.drive
        .rename(&remote("b"), &NodeId::Top, "b2.txt")
        .await
        .expect("rename");
    changed_there(&rig, "a", "a.txt", b"new bytes", "v2");
    changed_there(&rig, "b", "renamed there.txt", b"b", "v2");
    rig.drive.listing_changed();
    let mut names = rig.names(&NodeId::Top);
    names.sort();
    assert_eq!(names, ["a2.txt", "renamed there.txt"]);
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(
        rig.sink.took(),
        [Took::Relocate("a".into(), None, "a2.txt".into())]
    );
}

#[tokio::test]
async fn a_name_taken_on_the_drive_meets_the_folder_there_or_keeps_this_computers_file() {
    let rig = rig().await;
    rig.drive.set_paused(true);
    let work = rig.drive.make_folder(&NodeId::Top, "Work").expect("mkdir");
    let inside = rig.write_new(&work, "in.txt", b"in").await;
    let file = rig.write_new(&NodeId::Top, "a.txt", b"mine").await;
    // Made on the drive by someone else meanwhile.
    rig.listing.folder("w", None, "Work");
    rig.file("x", None, "a.txt", b"theirs");
    rig.drive.set_paused(false);
    rig.sent(2).await;
    assert_eq!(rig.drive.canonical(&work), remote("w"));
    let took = rig.sink.took();
    assert!(
        took.contains(&Took::New(Some("w".into()), "in.txt".into(), 2)),
        "{took:?}"
    );
    assert!(
        took.contains(&Took::New(None, "a (this computer).txt".into(), 4)),
        "{took:?}"
    );
    assert_eq!(
        rig.drive.path_of(&inside).expect("path"),
        Some("Work/in.txt".into())
    );
    let mine = rig.drive.status(&file).expect("status");
    assert_eq!(mine.state, ItemState::Conflict);
    assert_eq!(mine.other, Some(remote("x")));
}

#[tokio::test]
async fn the_desktop_trash_stays_here_and_sends_what_goes_in_to_the_recycle_bin() {
    let rig = rig().await;
    rig.file("r", None, "doc.txt", b"drive bytes");
    let trash = rig
        .drive
        .make_folder(&NodeId::Top, ".Trash-1000")
        .expect("trash");
    let files = rig.drive.make_folder(&trash, "files").expect("files");
    let info = rig.drive.make_folder(&trash, "info").expect("info");
    let note = rig
        .write_new(&info, "doc.txt.trashinfo", b"[Trash Info]")
        .await;
    rig.drive
        .rename(&remote("r"), &files, "doc.txt")
        .await
        .expect("to the trash");
    rig.sent(1).await;
    assert_eq!(rig.sink.took(), [Took::Trash("r".into())]);
    let kept = rig
        .drive
        .lookup(&files, "doc.txt")
        .expect("lookup")
        .expect("its bytes stay");
    assert_eq!(
        rig.drive.read(&kept.id, 0, 64).await.expect("read"),
        b"drive bytes"
    );
    assert_eq!(rig.state(&trash), ItemState::Downloaded);
    assert!(rig.drive.local_only(&kept.id));
    // Restored: sent again as a new file.
    rig.drive
        .rename(&kept.id, &NodeId::Top, "doc.txt")
        .await
        .expect("restore");
    rig.sent(2).await;
    assert_eq!(rig.sink.took()[1], Took::New(None, "doc.txt".into(), 11));
    // Emptied: only the bytes here go.
    rig.drive.remove(&note).expect("empty");
    rig.drive.remove_folder(&info).expect("empty");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(rig.sink.took().len(), 2);
}

/// Waits until `id` is being sent.
async fn uploading(rig: &Rig, id: &NodeId) {
    eventually("uploading", || rig.state(id) == ItemState::Uploading).await;
}

#[tokio::test]
async fn a_second_save_by_rename_during_an_upload_is_sent_too() {
    let rig = rig().await;
    rig.file("r", None, "doc.txt", b"0000000000");
    rig.sink.hold();
    let one = rig
        .write_new(&NodeId::Top, ".doc.txt.1", b"1111111111")
        .await;
    rig.drive
        .rename(&one, &NodeId::Top, "doc.txt")
        .await
        .expect("save");
    uploading(&rig, &remote("r")).await;
    // Written the same way as the first: as many writes, the same size.
    let two = rig
        .write_new(&NodeId::Top, ".doc.txt.2", b"2222222222")
        .await;
    rig.drive
        .rename(&two, &NodeId::Top, "doc.txt")
        .await
        .expect("save");
    rig.sink.release();
    eventually("the second save sent", || {
        rig.sink.uploads().last().map(Vec::as_slice) == Some(b"2222222222".as_slice())
    })
    .await;
    eventually("nothing left", || rig.drive.unsent() == 0).await;
    assert_eq!(
        rig.drive.read(&remote("r"), 0, 64).await.expect("read"),
        b"2222222222"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_while_its_upload_lands_is_never_lost() {
    let rig = rig().await;
    rig.file("r", None, "r.txt", b"0123456789");
    rig.sink.hold();
    rig.write_at("r", 0, b"AAAA").await;
    uploading(&rig, &remote("r")).await;
    let handle = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    let hook = Arc::new(std::sync::Barrier::new(2));
    *held(&rig.drive.inner.write_hook) = Some(Arc::clone(&hook));
    let drive = rig.drive.clone();
    let writing = tokio::spawn(async move { drive.write(handle, 4, b"BBBB").await });
    let meet = |hook: &Arc<std::sync::Barrier>| {
        let hook = Arc::clone(hook);
        tokio::task::spawn_blocking(move || {
            hook.wait();
        })
    };
    // The write has put its bytes down and not yet said so.
    meet(&hook).await.expect("met");
    *held(&rig.drive.inner.write_hook) = None;
    rig.sink.release();
    tokio::time::sleep(Duration::from_millis(100)).await;
    meet(&hook).await.expect("met");
    writing.await.expect("joined").expect("write");
    rig.drive.release_write(handle);
    eventually("sent again with the write", || {
        rig.sink.uploads().last().map(Vec::as_slice) == Some(b"AAAABBBB89".as_slice())
    })
    .await;
    eventually("nothing left", || rig.drive.unsent() == 0).await;
    let rig = restarted(rig).await;
    assert_eq!(
        rig.drive.read(&remote("r"), 0, 64).await.expect("read"),
        b"AAAABBBB89"
    );
}

#[tokio::test]
async fn what_is_written_during_an_upload_never_mixes_into_what_it_sends() {
    let rig = rig().await;
    rig.file("r", None, "r.txt", b"0123456789");
    rig.sink.hold();
    rig.write_at("r", 0, b"AAAA").await;
    uploading(&rig, &remote("r")).await;
    // Past what the upload has sent so far.
    rig.write_at("r", 8, b"ZZ").await;
    rig.sink.release();
    eventually("both sent", || rig.sink.uploads().len() == 2).await;
    let uploads = rig.sink.uploads();
    assert_eq!(
        uploads[0], b"AAAA456789",
        "the bytes as they were when it began"
    );
    assert_eq!(uploads[1], b"AAAA4567ZZ");
}

#[tokio::test]
async fn bytes_changed_after_an_upload_was_cut_off_start_a_new_session() {
    let rig = rig().await;
    let bytes: Vec<u8> = (0..40u8).collect();
    rig.sink.cut_after(2);
    let id = rig.write_new(&NodeId::Top, "big.bin", &bytes).await;
    eventually("cut off", || rig.sink.cuts() == 1).await;
    let handle = rig.drive.open_write(&id, false).await.expect("open");
    rig.drive.write(handle, 0, b"XY").await.expect("write");
    rig.drive.release_write(handle);
    eventually("sent", || !rig.sink.uploads().is_empty()).await;
    let mut wanted = bytes.clone();
    wanted[..2].copy_from_slice(b"XY");
    assert_eq!(rig.sink.uploads(), [wanted], "never old bytes and new");
    assert_eq!(rig.sink.abandoned(), 1, "the old session is given up");
}

#[tokio::test]
async fn a_file_open_to_write_when_the_drive_changed_it_writes_this_computers_version() {
    let rig = rig().await;
    rig.file("r", None, "Plan.txt", b"old");
    rig.drive.set_paused(true);
    let handle = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    rig.drive.write(handle, 0, b"mine").await.expect("write");
    let ino = rig.drive.ino(&remote("r"));
    changed_there(&rig, "r", "Plan.txt", b"theirs", "v2");
    rig.drive.listing_changed();
    rig.drive.write(handle, 4, b" too").await.expect("write");
    let copy = rig.drive.written(handle).expect("it writes somewhere");
    assert_ne!(copy, remote("r"));
    assert_eq!(rig.drive.ino(&copy), ino, "the file it opened");
    assert_ne!(rig.drive.ino(&remote("r")), ino);
    assert_eq!(
        rig.drive.read(&copy, 0, 64).await.expect("read"),
        b"mine too"
    );
    assert_eq!(
        rig.drive.read(&remote("r"), 0, 64).await.expect("read"),
        b"theirs"
    );
    rig.drive.release_write(handle);
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(
        rig.sink.took(),
        [Took::New(None, "Plan (this computer).txt".into(), 8)]
    );
}

#[tokio::test]
async fn a_file_open_to_write_when_the_drive_removed_it_keeps_writing() {
    let rig = rig().await;
    rig.file("r", None, "Notes.txt", b"old");
    // An empty listing is never taken as the drive's: something else stays.
    rig.file("k", None, "Kept.txt", b"k");
    rig.drive.set_paused(true);
    let handle = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    rig.drive.write(handle, 3, b" new").await.expect("write");
    rig.listing.remove("r");
    rig.drive.listing_changed();
    rig.drive
        .write(handle, 7, b"er")
        .await
        .expect("still writes");
    rig.drive.release_write(handle);
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(rig.sink.took(), [Took::New(None, "Notes.txt".into(), 9)]);
}

#[tokio::test]
async fn an_item_removed_while_it_is_sent_goes_to_the_recycle_bin_once_there() {
    let rig = rig().await;
    rig.sink.hold();
    let id = rig.write_new(&NodeId::Top, "gone.txt", b"0123456789").await;
    uploading(&rig, &id).await;
    rig.drive.remove(&id).expect("remove");
    rig.sink.release();
    rig.sent(2).await;
    let took = rig.sink.took();
    assert_eq!(took[0], Took::New(None, "gone.txt".into(), 10));
    assert!(matches!(took[1], Took::Trash(_)), "{took:?}");
    assert_eq!(rig.sink.bin(), ["gone.txt"]);
    assert!(rig.names(&NodeId::Top).is_empty());
}

#[tokio::test]
async fn a_file_opened_to_write_offline_or_emptied_downloads_nothing() {
    let rig = rig().await;
    rig.file("r", None, "r.txt", b"drive bytes");
    rig.source.set_offline(true);
    rig.drive.set_paused(true);
    let handle = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("opens offline");
    rig.drive.release_write(handle);
    let handle = rig
        .drive
        .open_write(&remote("r"), true)
        .await
        .expect("empties offline");
    rig.drive.write(handle, 0, b"new").await.expect("write");
    rig.drive.release_write(handle);
    assert_eq!(rig.source.fetched(), []);
    assert_eq!(
        rig.drive.read(&remote("r"), 0, 64).await.expect("read"),
        b"new"
    );
}

#[tokio::test]
async fn the_folder_of_bytes_made_here_is_synced_before_the_index_names_them() {
    let rig = rig().await;
    rig.drive.set_paused(true);
    let id = rig.write_new(&NodeId::Top, "n.txt", b"n").await;
    rig.drive.sync(&id).expect("sync");
    let synced = held(&rig.drive.inner.synced).clone();
    assert!(
        synced.contains(&rig.dir.join("cache").join("local")),
        "{synced:?}"
    );
}

#[tokio::test]
async fn a_file_removed_while_open_stays_usable_until_it_is_closed() {
    let rig = rig().await;
    rig.file("r", None, "r.txt", b"old");
    let handle = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    rig.drive.write(handle, 0, b"new").await.expect("write");
    rig.drive.remove(&remote("r")).expect("unlink");
    assert!(rig.names(&NodeId::Top).is_empty());
    rig.drive
        .write(handle, 3, b"er")
        .await
        .expect("writes after unlink");
    assert_eq!(
        rig.drive.read(&remote("r"), 0, 64).await.expect("read"),
        b"newer"
    );
    rig.drive.release_write(handle);
    rig.sent(1).await;
    assert_eq!(rig.sink.took(), [Took::Trash("r".into())]);
    let local = rig.dir.join("cache").join("local");
    eventually("its bytes go", || {
        std::fs::read_dir(&local).map_or(0, |dir| dir.count()) == 0
    })
    .await;
}

#[tokio::test]
async fn what_had_a_file_open_before_it_was_saved_over_writes_nowhere() {
    let rig = rig().await;
    rig.file("r", None, "doc.txt", b"old");
    rig.drive.set_paused(true);
    let before = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    let tmp = rig.write_new(&NodeId::Top, ".doc.txt.tmp", b"new").await;
    rig.drive
        .rename(&tmp, &NodeId::Top, "doc.txt")
        .await
        .expect("save");
    assert_eq!(
        rig.drive.write(before, 0, b"XXX").await,
        Err(WriteError::Gone)
    );
    assert_eq!(
        rig.drive.read(&remote("r"), 0, 64).await.expect("read"),
        b"new"
    );
    rig.drive.release_write(before);
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(rig.sink.uploads(), [b"new".to_vec()]);
}

#[tokio::test]
async fn this_computers_version_is_named_among_the_drives_names() {
    let rig = rig().await;
    // Shown as "a／b (this computer).txt"; the drive's name has the slash.
    rig.listing
        .file("t", None, "a/b (this computer).txt", 1, "v1");
    rig.file("r", None, "a/b.txt", b"old");
    rig.drive.set_paused(true);
    rig.write_at("r", 0, b"mine").await;
    changed_there(&rig, "r", "a/b.txt", b"theirs", "v2");
    rig.drive.listing_changed();
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(
        rig.sink.took(),
        [Took::New(None, "a/b (this computer) 2.txt".into(), 4)]
    );
}

#[tokio::test]
async fn what_named_an_item_made_here_goes_once_it_is_settled() {
    let rig = rig().await;
    rig.write_new(&NodeId::Top, "n.txt", b"n").await;
    rig.sent(1).await;
    for _ in 0..3 {
        rig.drive.listing_changed();
    }
    eventually("settled", || held(&rig.drive.inner.index).edits.is_empty()).await;
    let rig = restarted(rig).await;
    let index = held(&rig.drive.inner.index);
    assert!(index.aliases.is_empty(), "{:?}", index.aliases);
    assert!(index.inos.is_empty(), "{:?}", index.inos);
}

#[tokio::test]
async fn a_new_file_whose_answer_was_lost_is_taken_as_made_and_never_made_twice() {
    let rig = rig().await;
    rig.source.set_hashes(porter_capability::HashKind::Md5);
    rig.sink.lose_next();
    rig.write_new(&NodeId::Top, "a.txt", b"one two").await;
    eventually("it is sent again", || {
        rig.sink.tries() >= 2 && rig.drive.unsent() == 0
    })
    .await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(rig.sink.took(), [Took::New(None, "a.txt".into(), 7)]);
    assert_eq!(rig.names(&NodeId::Top), ["a.txt"]);
}

#[tokio::test]
async fn new_bytes_whose_answer_was_lost_are_taken_as_sent_not_split_off() {
    let rig = rig().await;
    rig.source.set_hashes(porter_capability::HashKind::Md5);
    rig.file("r", None, "r.txt", b"old");
    rig.sink.lose_next();
    rig.write_at("r", 0, b"new bytes").await;
    eventually("it is sent again", || {
        rig.sink.tries() >= 2 && rig.drive.unsent() == 0
    })
    .await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(rig.sink.took(), [Took::Replace("r".into(), 9)]);
    assert_eq!(rig.names(&NodeId::Top), ["r.txt"]);
}

#[tokio::test]
async fn what_is_made_under_a_reserved_id_is_made_once_however_often_it_is_sent() {
    let rig = rig().await;
    rig.source.set_hashes(porter_capability::HashKind::Md5);
    // A drive that takes a second item of a name: only the id tells a retry from a new item.
    rig.sink.twins();
    rig.sink.reserving();
    rig.sink.lose_next();
    let work = rig.drive.make_folder(&NodeId::Top, "Work").expect("folder");
    eventually("it is sent again", || {
        rig.sink.tries() >= 2 && rig.drive.unsent() == 0
    })
    .await;
    rig.sink.lose_next();
    let work = rig.drive.inner.canonical(&work);
    rig.write_new(&work, "a.txt", b"a").await;
    eventually("it is sent again", || {
        rig.sink.tries() >= 4 && rig.drive.unsent() == 0
    })
    .await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    let NodeId::Remote(folder) = &work else {
        panic!("{work:?}")
    };
    assert_eq!(
        rig.sink.took(),
        [
            Took::Folder(None, "Work".into()),
            Took::New(Some(folder.0.clone()), "a.txt".into(), 1)
        ]
    );
    assert_eq!(rig.names(&work), ["a.txt"]);
}

#[tokio::test]
async fn a_sign_in_the_drive_refuses_holds_what_waits_until_it_is_signed_in_again() {
    let rig = rig().await;
    rig.sink.refuse(Some(Refuse::SignedOut));
    rig.write_new(&NodeId::Top, "a.txt", b"a").await;
    eventually("held", || rig.drive.signed_out()).await;
    // Longer than the first wait after a change the drive could not take.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(rig.sink.tries(), 1, "nothing more is sent meanwhile");
    rig.sink.refuse(None);
    rig.drive.set_signed_out(false);
    rig.sent(1).await;
}

#[tokio::test]
async fn a_file_in_use_on_the_drive_waits_with_a_reason_that_says_so() {
    let rig = rig().await;
    rig.sink.refuse(Some(Refuse::InUse));
    let id = rig.write_new(&NodeId::Top, "a.txt", b"a").await;
    eventually("it waits with a reason", || {
        rig.drive.status(&id).is_some_and(|status| {
            status.state == ItemState::Waiting && status.reason == Some(Reason::InUse)
        })
    })
    .await;
    rig.sink.refuse(None);
    rig.sent(1).await;
}

#[tokio::test]
async fn a_name_the_drive_refuses_and_a_change_it_declines_say_what_they_are() {
    let rig = rig().await;
    rig.drive.set_paused(true);
    let bad = rig.write_new(&NodeId::Top, "a#b.txt", b"a").await;
    rig.sink.names(|name| !name.contains('#'));
    rig.drive.set_paused(false);
    let reason_of = |id: &NodeId| rig.drive.status(id).and_then(|status| status.reason);
    eventually("refused", || reason_of(&bad) == Some(Reason::NameRefused)).await;
    rig.sink.refuse(Some(Refuse::Declined));
    let declined = rig.write_new(&NodeId::Top, "c.txt", b"c").await;
    eventually("declined", || {
        reason_of(&declined) == Some(Reason::Declined)
    })
    .await;
}

#[tokio::test]
async fn a_path_longer_than_the_drive_takes_is_refused_where_it_is_made() {
    let rig = rig().await;
    rig.drive.set_paused(true);
    rig.sink.set_path_max(12);
    let work = rig.drive.make_folder(&NodeId::Top, "Work").expect("folder");
    let (_, handle) = rig.drive.create(&work, "1234567").expect("12 characters");
    rig.drive.release_write(handle);
    assert_eq!(
        rig.drive.create(&work, "12345678").map(drop),
        Err(WriteError::BadName)
    );
    assert_eq!(
        rig.drive.make_folder(&work, "12345678").map(drop),
        Err(WriteError::BadName)
    );
    assert_eq!(
        rig.drive.rename(&work, &NodeId::Top, "Works").await,
        Err(WriteError::BadName),
        "what is inside a folder moves with it"
    );
}

#[tokio::test]
async fn a_rename_onto_a_name_the_drive_gave_away_meanwhile_takes_the_next_number() {
    let rig = rig().await;
    rig.file("a", None, "a.txt", b"a");
    rig.drive.set_paused(true);
    rig.drive
        .rename(&remote("a"), &NodeId::Top, "b.txt")
        .await
        .expect("rename");
    // Someone else's on the drive, there before this rename is sent.
    rig.file("b", None, "b.txt", b"b");
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(
        rig.sink.took(),
        [Took::Relocate("a".into(), None, "b 2.txt".into())]
    );
    assert_eq!(rig.sink.tries(), 2);
}

#[tokio::test]
async fn a_pause_stops_an_upload_at_its_next_chunk_and_it_goes_on_from_there() {
    let rig = rig().await;
    rig.sink.hold();
    let id = rig
        .write_new(&NodeId::Top, "big.bin", b"0123456789abcdef")
        .await;
    uploading(&rig, &id).await;
    rig.drive.set_paused(true);
    rig.sink.release();
    eventually("it stops", || rig.state(&id) == ItemState::Waiting).await;
    assert!(rig.sink.took().is_empty());
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(rig.sink.read_bytes(), 16, "no byte is sent twice");
    assert_eq!(rig.sink.uploads(), [b"0123456789abcdef".to_vec()]);
}

#[tokio::test]
async fn what_was_not_sent_is_written_out_as_plain_files_when_the_drive_is_let_go() {
    let rig = rig().await;
    rig.listing.folder("f", None, "Work");
    rig.file("r", Some("f"), "report.txt", b"old text");
    rig.drive.set_paused(true);
    rig.write_at("r", 0, b"new").await;
    let work = remote("f");
    rig.write_new(&work, "made.txt", b"made here").await;
    let plans = rig
        .drive
        .make_folder(&NodeId::Top, "Plans")
        .expect("folder");
    rig.write_new(&plans, "a.txt", b"a").await;
    rig.drive
        .make_folder(&NodeId::Top, "Empty")
        .expect("folder");
    rig.write_new(&NodeId::Top, ".~lock.a.txt#", b"lock").await;
    let into = rig.dir.join("Cloud").join("Drive (Not Uploaded)");
    std::fs::create_dir_all(into.join("Work")).expect("folder");
    std::fs::write(into.join("Work").join("made.txt"), b"there").expect("write");
    assert_eq!(rig.drive.keep_unsent(&into).expect("kept"), 3);
    // Asked again (a removal tried again while the cache is still here): nothing twice.
    assert_eq!(rig.drive.keep_unsent(&into).expect("kept"), 3);
    assert!(!into.join("Work/made 3.txt").exists());
    assert!(!into.join("Work/report 2.txt").exists());
    assert!(
        !into.join(".~lock.a.txt#").exists(),
        "a lock file is left out"
    );
    let read = |path: &str| std::fs::read(into.join(path)).expect(path);
    assert_eq!(read("Work/report.txt"), b"new text");
    assert_eq!(read("Work/made 2.txt"), b"made here");
    assert_eq!(read("Work/made.txt"), b"there", "what was there stays");
    assert_eq!(read("Plans/a.txt"), b"a");
    assert!(into.join("Empty").is_dir());
}

/// Meets a test's hold inside the drive once.
fn meet(hook: &Arc<std::sync::Barrier>) -> tokio::task::JoinHandle<()> {
    let hook = Arc::clone(hook);
    tokio::task::spawn_blocking(move || {
        hook.wait();
    })
}

#[tokio::test]
async fn bytes_written_between_two_tries_under_a_reserved_id_are_sent_over_what_the_first_made() {
    let rig = rig().await;
    rig.source.set_hashes(porter_capability::HashKind::Md5);
    rig.sink.reserving();
    rig.sink.lose_next();
    let id = rig.write_new(&NodeId::Top, "a.txt", b"first").await;
    eventually("the first try", || rig.sink.tries() >= 1).await;
    // Written again before the next try, whose bytes the drive does not hold.
    let handle = rig.drive.open_write(&id, false).await.expect("open");
    rig.drive
        .write(handle, 0, b"FIRST, then more")
        .await
        .expect("write");
    rig.drive.release_write(handle);
    eventually("sent", || {
        rig.sink.uploads().len() >= 2 && rig.drive.unsent() == 0
    })
    .await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        rig.sink.uploads(),
        [b"first".to_vec(), b"FIRST, then more".to_vec()]
    );
    let took = rig.sink.took();
    assert_eq!(took[0], Took::New(None, "a.txt".into(), 5));
    assert!(matches!(&took[1], Took::Replace(_, 16)), "{took:?}");
    assert_eq!(took.len(), 2, "{took:?}");
}

#[tokio::test]
async fn a_new_file_is_never_taken_for_a_drive_item_another_change_here_is_of() {
    let rig = rig().await;
    rig.source.set_hashes(porter_capability::HashKind::Md5);
    rig.file("r", None, "a.txt", b"same");
    rig.file("k", None, "Kept.txt", b"k");
    rig.sink.refuse(Some(Refuse::Forbidden));
    rig.drive
        .rename(&remote("r"), &NodeId::Top, "b.txt")
        .await
        .expect("rename");
    eventually("refused", || {
        rig.drive
            .inner
            .edit(&remote("r"))
            .is_some_and(|edit| edit.refused.is_some())
    })
    .await;
    rig.sink.refuse(None);
    // The drive's "a.txt" holds just these bytes, and is the file renamed here.
    rig.write_new(&NodeId::Top, "a.txt", b"same").await;
    eventually("tried", || rig.sink.tries() >= 2).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let edit = rig
        .drive
        .inner
        .edit(&remote("r"))
        .expect("the rename stays");
    assert_eq!(edit.name, "b.txt");
    eventually("sent as a file of its own", || {
        rig.sink
            .took()
            .iter()
            .any(|took| matches!(took, Took::New(None, _, 4)))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_through_a_file_saved_over_never_lands_in_the_bytes_that_replaced_it() {
    let rig = rig().await;
    rig.file("r", None, "r.txt", b"old");
    rig.drive.set_paused(true);
    let handle = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    rig.drive.write(handle, 0, b"OLD").await.expect("write");
    let new = rig.write_new(&NodeId::Top, "r.new", b"NEW").await;
    let hook = Arc::new(std::sync::Barrier::new(2));
    held(&rig.drive.inner.hooks).insert("saved over", Arc::clone(&hook));
    let drive = rig.drive.clone();
    let saving = tokio::spawn(async move { drive.rename(&new, &NodeId::Top, "r.txt").await });
    // Saved over, its handles shut, the bytes' locks still held: a write waits for them.
    meet(&hook).await.expect("met");
    let drive = rig.drive.clone();
    let late = tokio::spawn(async move { drive.write(handle, 0, b"ZZZ").await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    held(&rig.drive.inner.hooks).remove("saved over");
    meet(&hook).await.expect("met");
    saving.await.expect("joined").expect("saved");
    assert_eq!(late.await.expect("joined"), Err(WriteError::Gone));
    rig.drive.release_write(handle);
    rig.drive.set_paused(false);
    eventually("sent", || rig.drive.unsent() == 0).await;
    assert_eq!(rig.sink.uploads(), [b"NEW".to_vec()]);
}

#[tokio::test]
async fn a_file_removed_while_open_is_still_written_and_never_sent() {
    let rig = rig().await;
    rig.file("r", None, "r.txt", b"old");
    rig.file("k", None, "Kept.txt", b"k");
    rig.drive.set_paused(true);
    let handle = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    // `exec 3<>r.txt; rm r.txt; echo x >&3`
    rig.drive.remove(&remote("r")).expect("remove");
    assert_eq!(rig.drive.write(handle, 3, b"x").await, Ok(1));
    rig.drive.release_write(handle);
    rig.drive.set_paused(false);
    rig.sent(1).await;
    assert_eq!(rig.sink.took(), [Took::Trash("r".into())]);
}

#[tokio::test]
async fn a_reader_of_a_file_the_drive_removed_closes_what_it_opened() {
    let rig = rig().await;
    rig.file("r", None, "Notes.txt", b"old");
    rig.file("k", None, "Kept.txt", b"k");
    rig.drive.set_paused(true);
    rig.write_at("r", 3, b" new").await;
    rig.drive.open_file(&remote("r")).expect("open to read");
    rig.listing.remove("r");
    rig.drive.listing_changed();
    assert!(
        rig.drive
            .lookup(&NodeId::Top, "Notes.txt")
            .expect("lookup")
            .is_some(),
        "kept as a new file"
    );
    rig.drive.release(&remote("r"));
    let open = held(&rig.drive.inner.open).clone();
    assert!(open.is_empty(), "{open:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_never_waits_for_the_copy_an_upload_takes() {
    let rig = rig().await;
    rig.file("r", None, "r.txt", b"0123456789");
    let hook = Arc::new(std::sync::Barrier::new(2));
    held(&rig.drive.inner.hooks).insert("snapshot", Arc::clone(&hook));
    rig.write_at("r", 0, b"AAAA").await;
    // The copy is taken; the bytes' lock is not.
    meet(&hook).await.expect("met");
    let handle = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    let wrote =
        tokio::time::timeout(Duration::from_secs(5), rig.drive.write(handle, 4, b"BBBB")).await;
    held(&rig.drive.inner.hooks).remove("snapshot");
    meet(&hook).await.expect("met");
    assert_eq!(wrote.expect("not held up by the copy"), Ok(4));
    rig.drive.release_write(handle);
    eventually("sent with the write", || rig.drive.unsent() == 0).await;
    assert_eq!(
        rig.sink.uploads(),
        [b"AAAABBBB89".to_vec()],
        "the copy of older bytes is not sent"
    );
}

#[tokio::test]
async fn a_file_this_computer_has_no_room_to_copy_for_its_upload_says_so() {
    let rig = rig().await;
    held(&rig.drive.inner.faults).insert("snapshot", std::io::ErrorKind::StorageFull);
    let id = rig.write_new(&NodeId::Top, "a.txt", b"a").await;
    eventually("said", || {
        rig.drive.status(&id).and_then(|status| status.reason) == Some(Reason::NoRoomHere)
    })
    .await;
    held(&rig.drive.inner.faults).clear();
    rig.sent(1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_older_copy_of_the_index_is_never_written_over_a_newer_one() {
    let rig = rig().await;
    rig.drive.set_paused(true);
    rig.drive
        .make_folder(&NodeId::Top, "First")
        .expect("folder");
    let hook = Arc::new(std::sync::Barrier::new(2));
    held(&rig.drive.inner.hooks).insert("flush", Arc::clone(&hook));
    let inner = Arc::clone(&rig.drive.inner);
    let older = tokio::task::spawn_blocking(move || inner.flush_now());
    // The first write has taken its copy, without "Second".
    meet(&hook).await.expect("met");
    rig.drive
        .make_folder(&NodeId::Top, "Second")
        .expect("folder");
    let inner = Arc::clone(&rig.drive.inner);
    let newer = tokio::task::spawn_blocking(move || inner.flush_now());
    tokio::time::sleep(Duration::from_millis(100)).await;
    held(&rig.drive.inner.hooks).remove("flush");
    meet(&hook).await.expect("met");
    older.await.expect("joined").expect("written");
    newer.await.expect("joined").expect("written");
    let stored = std::fs::read_to_string(rig.dir.join("cache").join("index.json")).expect("index");
    assert!(stored.contains("Second"), "{stored}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_never_waits_for_a_copy_into_the_trash() {
    let rig = rig().await;
    rig.file("r", None, "r.txt", b"0123456789");
    rig.file("k", None, "Kept.txt", b"k");
    rig.drive.set_paused(true);
    rig.write_at("r", 0, b"AAAA").await;
    let trash = rig
        .drive
        .make_folder(&NodeId::Top, ".Trash-1000")
        .expect("trash");
    let hook = Arc::new(std::sync::Barrier::new(2));
    held(&rig.drive.inner.hooks).insert("copied here", Arc::clone(&hook));
    let drive = rig.drive.clone();
    let moving = tokio::spawn(async move { drive.rename(&remote("r"), &trash, "r.txt").await });
    // The copy is made; the bytes' lock is not taken.
    meet(&hook).await.expect("met");
    let handle = rig
        .drive
        .open_write(&remote("r"), false)
        .await
        .expect("open");
    let wrote =
        tokio::time::timeout(Duration::from_secs(5), rig.drive.write(handle, 4, b"BBBB")).await;
    held(&rig.drive.inner.hooks).remove("copied here");
    meet(&hook).await.expect("met");
    assert_eq!(wrote.expect("not held up by the copy"), Ok(4));
    rig.drive.release_write(handle);
    moving.await.expect("joined").expect("moved");
    // Taken again with the write: the trash holds the bytes as they are now.
    let kept = rig
        .drive
        .lookup(
            &rig.drive
                .lookup(&NodeId::Top, ".Trash-1000")
                .expect("lookup")
                .expect("trash")
                .id,
            "r.txt",
        )
        .expect("lookup")
        .expect("in the trash");
    assert_eq!(
        rig.drive.read(&kept.id, 0, 10).await.expect("read"),
        b"AAAABBBB89"
    );
}

/// The cache's lock goes with the dropped drive's last sender task, which runs on the very
/// runtime that opens the next drive. On a current-thread runtime a blocking wait would keep
/// that task from ever running; the wait must yield, so the reopen succeeds within `lock_wait`.
#[tokio::test(flavor = "current_thread")]
async fn a_drive_reopened_on_a_one_thread_runtime_gets_the_lock_when_the_old_one_lets_go() {
    let rig = rig().await;
    // Sending is still to do when the drive is dropped, so its sender task has work left.
    rig.write_new(&NodeId::Top, "pending.txt", b"still to send")
        .await;
    let Rig {
        dir,
        listing,
        source,
        sink,
        drive,
    } = rig;
    drop(drive);
    let wait = Duration::from_secs(10);
    let began = std::time::Instant::now();
    let again = Drive::open_writable(
        DriveConfig {
            lock_wait: wait,
            ..config(&dir)
        },
        listing,
        source,
        sink,
    )
    .await;
    assert!(again.is_ok(), "the reopen got the lock: {:?}", again.err());
    assert!(began.elapsed() < wait, "within the lock wait");
}
