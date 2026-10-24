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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
    rig.drive.set_paused(true);
    let id = rig.write_new(&NodeId::Top, "kept.txt", b"keep me").await;
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
    let id = rig.write_new(&NodeId::Top, "big.bin", &bytes).await;
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
    let id = rig.write_new(&NodeId::Top, "big.txt", b"abc").await;
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
    let lock = rig.write_new(&NodeId::Top, "~$Plan.docx", b"me").await;
    let other = rig.write_new(&NodeId::Top, ".~lock.Plan.odt#", b"me").await;
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
    rig.write_at("r", 0, b"DRIVE").await;
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
    let drive = Drive::open(config(&dir), listing, source).expect("drive");
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
    let rig = rig();
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
    assert_eq!(rig.drive.remove_download(&copy), Err(words::CONFLICT));
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = restarted(rig);
    assert_eq!(
        rig.drive.read(&remote("r"), 0, 64).await.expect("read"),
        b"AAAABBBB89"
    );
}

#[tokio::test]
async fn what_is_written_during_an_upload_never_mixes_into_what_it_sends() {
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
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
    let rig = rig();
    rig.write_new(&NodeId::Top, "n.txt", b"n").await;
    rig.sent(1).await;
    for _ in 0..3 {
        rig.drive.listing_changed();
    }
    eventually("settled", || held(&rig.drive.inner.index).edits.is_empty()).await;
    let rig = restarted(rig);
    let index = held(&rig.drive.inner.index);
    assert!(index.aliases.is_empty(), "{:?}", index.aliases);
    assert!(index.inos.is_empty(), "{:?}", index.inos);
}
