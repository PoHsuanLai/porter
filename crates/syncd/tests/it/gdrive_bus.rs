//! syncd's storage supervisor against the real accountd and the fake Google's Drive app data
//! folder on one private bus. A Storage grant on a Google account (class Files) becomes the
//! dataset that mirrors the app data folder both ways: local adds, edits, renames and deletes go
//! up; remote ones come down; a two-sided edit is a `Conflict` signal with its number and
//! `Resolve` settles it; a large file goes up in a resumable session; the changes cursor
//! survives a restart; the dataset follows the grant. The relay authenticates: no token or
//! secret is in any bus message or any file syncd keeps.

use crate::common;
use crate::googlerig as google_rig;

use common::eventually;
use google_rig::{
    DRIVE_BEARER, FILES_DATASET, Rig, SECRET_ACCESS, SECRET_REFRESH, SEGMENT, client_of, local, rig,
};
use porter_dbus::{
    CONFLICT_KEY_NUMBER, RESOLVE_KEEP_LOCAL, RESOLVE_KEEP_REMOTE, SYNC_ERROR_NO_SUCH_CONFLICT,
    SyncProxy,
};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::time::Duration;
use syncd::datasets::photos::PhotosSwitch;
use syncd::datasets::storage::FILES_APP;
use zbus::export::futures_core::Stream;

async fn names(sync: &SyncProxy<'_>) -> Vec<String> {
    sync.datasets().await.expect("datasets")
}

fn remote_is(rig: &Rig, path: &str, bytes: &[u8]) -> bool {
    rig.google.drive_file(path).as_deref() == Some(bytes)
}

fn local_is(rig: &Rig, path: &str, bytes: &[u8]) -> bool {
    std::fs::read(local(rig, path)).ok().as_deref() == Some(bytes)
}

/// How many requests of the fake Drive so far were this method on a path containing `part`.
fn count(rig: &Rig, method: &str, part: &str) -> usize {
    rig.google
        .hits()
        .iter()
        .filter(|h| h.method == method && h.target.contains(part))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_files_grant_on_a_google_account_mirrors_the_app_data_folder_both_ways_through_the_relay()
{
    let mut rig = rig(PhotosSwitch::Off).await;
    let files = client_of(&rig, FILES_APP).await;
    let sync = SyncProxy::new(&files).await.expect("proxy");
    assert_eq!(names(&sync).await, Vec::<String>::new(), "nothing before");

    rig.supervisor.tick().await;
    assert_eq!(
        names(&sync).await,
        [FILES_DATASET],
        "the grant became a dataset"
    );

    // Up: a file in a folder, then an edit, then a rename, then a delete.
    std::fs::create_dir_all(local(&rig, "notes")).expect("dir");
    std::fs::write(local(&rig, "notes/a.txt"), b"from here").expect("write");
    eventually("the file is on the drive", || {
        remote_is(&rig, "notes/a.txt", b"from here")
    })
    .await;
    std::fs::write(local(&rig, "notes/a.txt"), b"edited here").expect("edit");
    eventually("the edit is on the drive", || {
        remote_is(&rig, "notes/a.txt", b"edited here")
    })
    .await;
    std::fs::rename(local(&rig, "notes/a.txt"), local(&rig, "notes/c.txt")).expect("rename");
    eventually("the rename is on the drive", || {
        remote_is(&rig, "notes/c.txt", b"edited here")
            && rig.google.drive_file("notes/a.txt").is_none()
    })
    .await;
    std::fs::remove_file(local(&rig, "notes/c.txt")).expect("delete");
    eventually("the delete is on the drive", || {
        rig.google.drive_file("notes/c.txt").is_none()
    })
    .await;

    // Down: an add, an edit, a delete.
    rig.google.drive_put_file("from-phone/b.txt", b"from there");
    eventually("the remote file arrives", || {
        local_is(&rig, "from-phone/b.txt", b"from there")
    })
    .await;
    rig.google
        .drive_put_file("from-phone/b.txt", b"changed there");
    eventually("the remote edit arrives", || {
        local_is(&rig, "from-phone/b.txt", b"changed there")
    })
    .await;
    rig.google.drive_delete("from-phone/b.txt");
    eventually("a remote delete is a local delete", || {
        !local(&rig, "from-phone/b.txt").exists()
    })
    .await;

    // The relay authenticated every request, and only the Drive's bearer was ever presented.
    let want = format!("Bearer {DRIVE_BEARER}");
    let hits = rig.google.hits();
    assert!(!hits.is_empty());
    assert!(
        hits.iter()
            .all(|h| h.authorization.as_deref() == Some(want.as_str())),
        "{hits:?}"
    );
    // Small files went by multipart and the feed was changes.list, not a listing every time.
    assert!(count(&rig, "POST", "uploadType=multipart") >= 1);
    assert!(count(&rig, "GET", "/drive/v3/changes?") >= 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_two_sided_edit_is_a_conflict_signal_and_keep_remote_or_keep_local_settles_it() {
    let mut rig = rig(PhotosSwitch::Off).await;
    let files = client_of(&rig, FILES_APP).await;
    let sync = SyncProxy::new(&files).await.expect("proxy");
    let mut conflicts = sync.receive_conflict().await.expect("stream");
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, [FILES_DATASET]);

    for (file, how, local_wins) in [
        ("doc.txt", RESOLVE_KEEP_REMOTE, false),
        ("other.txt", RESOLVE_KEEP_LOCAL, true),
    ] {
        rig.google.drive_put_file(file, b"one");
        eventually("the file arrives", || local_is(&rig, file, b"one")).await;

        // Both sides edit within one pause, so no cycle sees one edit alone.
        sync.pause(FILES_DATASET).await.expect("pause");
        tokio::time::sleep(Duration::from_millis(400)).await;
        rig.google.drive_put_file(file, b"theirs");
        std::fs::write(local(&rig, file), b"mine").expect("local edit");
        sync.resume(FILES_DATASET).await.expect("resume");

        let signal = tokio::time::timeout(
            Duration::from_secs(120),
            std::future::poll_fn(|cx| Pin::new(&mut conflicts).poll_next(cx)),
        )
        .await
        .expect("a Conflict signal in time")
        .expect("open stream");
        let args = signal.args().expect("args");
        assert_eq!(args.dataset(), &FILES_DATASET);
        let number = i64::try_from(&args.conflict()[CONFLICT_KEY_NUMBER]).expect("a number");
        assert!(number >= 1, "the journal's own number: {number}");

        sync.resolve(FILES_DATASET, number, how)
            .await
            .expect("resolve");
        let (here, there): (&[u8], &[u8]) = match local_wins {
            true => (b"mine", b"mine"),
            false => (b"theirs", b"theirs"),
        };
        eventually("both sides agree", || {
            local_is(&rig, file, here) && remote_is(&rig, file, there)
        })
        .await;
        let err = sync
            .resolve(FILES_DATASET, number, RESOLVE_KEEP_REMOTE)
            .await
            .expect_err("settled already");
        assert!(
            format!("{err:?}").contains(SYNC_ERROR_NO_SUCH_CONFLICT),
            "{err:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_large_file_goes_up_in_a_resumable_session_and_a_small_one_does_not() {
    let mut rig = rig(PhotosSwitch::Off).await;
    rig.supervisor.tick().await;

    // Past the 4 000 000 byte threshold of one multipart request.
    let big: Vec<u8> = (0..4_300_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(local(&rig, "big.bin"), &big).expect("big");
    std::fs::write(local(&rig, "small.txt"), b"small").expect("small");
    eventually("both files are on the drive", || {
        remote_is(&rig, "big.bin", &big) && remote_is(&rig, "small.txt", b"small")
    })
    .await;
    assert_eq!(count(&rig, "POST", "uploadType=resumable"), 1);
    assert!(count(&rig, "PUT", "uploadType=resumable&upload_id=") >= 1);
    assert_eq!(
        count(&rig, "POST", "uploadType=multipart"),
        1,
        "the small file went by multipart, the large one did not"
    );
    let ended = rig
        .google
        .hits()
        .iter()
        .filter(|h| h.method == "PUT" && h.target.contains("upload_id="))
        .map(|h| h.status)
        .next_back();
    assert_eq!(ended, Some(200), "the session ended with the file");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_changes_cursor_survives_a_restart_and_an_expired_one_reconciles_without_re_uploads() {
    let mut rig = rig(PhotosSwitch::Off).await;
    let files = client_of(&rig, FILES_APP).await;
    let sync = SyncProxy::new(&files).await.expect("proxy");
    rig.supervisor.tick().await;
    std::fs::write(local(&rig, "mine.txt"), b"mine").expect("write");
    rig.google.drive_put_file("theirs.txt", b"theirs");
    eventually("both directions settle", || {
        remote_is(&rig, "mine.txt", b"mine") && local_is(&rig, "theirs.txt", b"theirs")
    })
    .await;

    // The daemon stops and starts again: its datasets leave the hub, a new supervisor finds the
    // journal and its anchor.
    rig.files_shown.store(false, Ordering::SeqCst);
    rig.photos_shown.store(false, Ordering::SeqCst);
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, Vec::<String>::new());
    rig.supervisor = rig.new_supervisor();
    rig.files_shown.store(true, Ordering::SeqCst);
    rig.photos_shown.store(true, Ordering::SeqCst);

    let before = rig.google.hits().len();
    let listings_before = count(&rig, "GET", "q=trashed%20%3D%20false");
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, [FILES_DATASET]);
    rig.google.drive_put_file("after.txt", b"after");
    eventually("a change made after the restart arrives", || {
        local_is(&rig, "after.txt", b"after")
    })
    .await;
    assert_eq!(
        count(&rig, "GET", "q=trashed%20%3D%20false"),
        listings_before,
        "the restart resumed from the anchor and listed nothing again"
    );
    assert!(rig.google.hits().len() > before);

    // The server drops its page tokens: the engine lists again and finds both sides in
    // agreement, so no file is sent a second time.
    let uploads = count(&rig, "POST", "/upload/drive/v3/");
    rig.google.drive_expire_tokens();
    rig.google.drive_put_file("post-expiry.txt", b"post");
    eventually("the feed recovers and brings the new file", || {
        local_is(&rig, "post-expiry.txt", b"post")
    })
    .await;
    assert!(
        count(&rig, "GET", "q=trashed%20%3D%20false") > listings_before,
        "an expired anchor means a full listing"
    );
    assert_eq!(
        count(&rig, "POST", "/upload/drive/v3/"),
        uploads,
        "nothing was uploaded again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_dataset_follows_the_grant_and_an_account_removal_wipes_the_mirror() {
    let mut rig = rig(PhotosSwitch::Off).await;
    let files = client_of(&rig, FILES_APP).await;
    let sync = SyncProxy::new(&files).await.expect("proxy");
    rig.supervisor.tick().await;
    rig.google.drive_put_file("keep.txt", b"kept");
    eventually("the file arrives", || local(&rig, "keep.txt").exists()).await;

    // A tick that finds the grant again changes nothing.
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, [FILES_DATASET]);

    rig.files_shown.store(false, Ordering::SeqCst);
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
    rig.google.drive_put_file("late.txt", b"late");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!local(&rig, "late.txt").exists());

    // The grant comes back: the folder is picked up again where it was.
    rig.files_shown.store(true, Ordering::SeqCst);
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, [FILES_DATASET]);
    eventually("the missed file arrives", || {
        local(&rig, "late.txt").exists()
    })
    .await;

    // AccountRemoved: the wipe takes the folder and the journal with it.
    rig.files_shown.store(false, Ordering::SeqCst);
    syncd::removal::wipe(&rig.paths, &rig.hub, &rig.account)
        .await
        .expect("wipe");
    rig.supervisor.tick().await;
    assert!(!rig.paths.storage_dir(&rig.account).exists());
    assert!(!rig.paths.journals.join(SEGMENT).exists());
    assert_eq!(names(&sync).await, Vec::<String>::new());
}

/// Every message on the bus, from the moment the monitor is set.
struct Tap(zbus::MessageStream);

impl Tap {
    async fn start(rig: &Rig) -> Self {
        let monitor = rig.bus.connect().await;
        zbus::fdo::MonitoringProxy::new(&monitor)
            .await
            .expect("proxy")
            .become_monitor(&[], 0)
            .await
            .expect("monitor");
        Self(zbus::MessageStream::from(monitor))
    }

    async fn drain(&mut self) -> Vec<Vec<u8>> {
        let mut seen = Vec::new();
        loop {
            let next = tokio::time::timeout(
                Duration::from_millis(300),
                std::future::poll_fn(|cx| Pin::new(&mut self.0).poll_next(cx)),
            )
            .await;
            match next {
                Ok(Some(Ok(message))) => seen.push(message.data().to_vec()),
                _ => return seen,
            }
        }
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// Every file below `dir`, whole.
fn files_below(dir: &std::path::Path, into: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => files_below(&path, into),
            Ok(kind) if kind.is_file() => {
                if let Ok(bytes) = std::fs::read(&path) {
                    into.push((path, bytes));
                }
            }
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn no_token_or_secret_is_on_the_bus_in_a_journal_or_in_anything_syncd_prints() {
    let mut rig = rig(PhotosSwitch::Off).await;
    let mut tap = Tap::start(&rig).await;
    let files = client_of(&rig, FILES_APP).await;
    let sync = SyncProxy::new(&files).await.expect("proxy");
    rig.supervisor.tick().await;
    std::fs::write(local(&rig, "a.txt"), b"a").expect("write");
    rig.google.drive_put_file("b.txt", b"b");
    eventually("both directions settle", || {
        remote_is(&rig, "a.txt", b"a") && local_is(&rig, "b.txt", b"b")
    })
    .await;
    let _ = sync.status(FILES_DATASET).await.expect("status");
    let _ = sync.datasets().await.expect("datasets");

    // The bus: no message carries the credential accountd holds or the bearer it adds.
    let seen = tap.drain().await;
    assert!(
        seen.len() > 10,
        "the monitor saw the traffic: {}",
        seen.len()
    );
    for secret in [SECRET_ACCESS, SECRET_REFRESH, DRIVE_BEARER, "Bearer "] {
        assert!(
            !seen.iter().any(|m| contains(m, secret)),
            "{secret} crossed the bus"
        );
    }
    assert!(
        seen.iter().any(|m| contains(m, FILES_DATASET)),
        "positive control: the scan finds a value that did cross"
    );

    // Everything syncd keeps on disk (journals, the mirror) and what it prints about itself.
    let mut kept = Vec::new();
    files_below(&rig.paths.journals, &mut kept);
    files_below(&rig.paths.storage_dir(&rig.account), &mut kept);
    assert!(kept.len() >= 3, "journal and mirror files: {}", kept.len());
    let printed = format!("{:?}", rig.supervisor);
    for secret in [SECRET_ACCESS, SECRET_REFRESH, DRIVE_BEARER] {
        assert!(
            !kept.iter().any(|(_, bytes)| contains(bytes, secret)),
            "{secret} is in a file syncd keeps"
        );
        assert!(
            !printed.contains(secret),
            "{secret} is in syncd's own debug text"
        );
    }
}
