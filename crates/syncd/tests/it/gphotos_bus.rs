//! syncd's Google Photos against the real accountd and the fake Google on one private bus:
//! behind the Photos switch, a Photos grant on a Google account becomes an upload dataset (new
//! files in the upload folder are sent once, into an album the app created, and nothing is
//! pulled) and a picker (session, polling, the picked items downloaded, the session deleted).
//! The relay authenticates each API with its own audience's bearer; no secret is anywhere.

use crate::common;
use crate::googlerig as google_rig;

use common::eventually;
use google_rig::{
    FILES_DATASET, Media, PICKER_BEARER, Rig, SECRET_ACCESS, SECRET_REFRESH, UPLOAD_BEARER,
    UPLOAD_DATASET, client_of, rig, rig_picker_only, rig_with,
};
use porter_dbus::SyncProxy;
use porter_fake_servers::google::Pick;
use porter_sync::ItemState;
use std::sync::atomic::Ordering;
use syncd::datasets::photos::PhotosSwitch;
use syncd::datasets::photos::google::{
    ALBUM_TITLE, PickerError, PickerState, SLUG, SessionEnd, SessionId,
};
use syncd::datasets::storage::{FILES_APP, PHOTOS_APP};
use syncd::journal::Journal;

async fn names(sync: &SyncProxy<'_>) -> Vec<String> {
    sync.datasets().await.expect("datasets")
}

fn upload_dir(rig: &Rig) -> std::path::PathBuf {
    rig.paths.photos_upload_dir(&rig.account)
}

/// The paths the upload dataset's journal holds as synced.
fn synced(rig: &Rig) -> Vec<String> {
    let Ok(journal) = Journal::open(&rig.paths.journal(&rig.account, SLUG)) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = journal
        .items()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.state == ItemState::Synced)
        .map(|item| item.path.0)
        .collect();
    paths.sort();
    paths
}

fn count(rig: &Rig, method: &str, part: &str) -> usize {
    rig.google
        .hits()
        .iter()
        .filter(|h| h.method == method && h.target.contains(part))
        .count()
}

/// A JPEG-ish payload, distinct per `n`.
fn photo(n: u8) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0];
    bytes.extend((0..2000u32).map(|i| (i as u8).wrapping_mul(n).wrapping_add(n)));
    bytes
}

#[tokio::test(flavor = "multi_thread")]
async fn google_photos_run_only_behind_the_switch() {
    // Off: the Photos grant starts nothing, makes no folder and has no picker.
    let mut off = rig(PhotosSwitch::Off).await;
    let photos = client_of(&off, PHOTOS_APP).await;
    let files = client_of(&off, FILES_APP).await;
    off.supervisor.tick().await;
    assert_eq!(
        names(&SyncProxy::new(&photos).await.expect("proxy")).await,
        Vec::<String>::new()
    );
    assert_eq!(
        names(&SyncProxy::new(&files).await.expect("proxy")).await,
        [FILES_DATASET],
        "the Drive mirror is not Photos' to switch"
    );
    assert!(off.supervisor.google_picker(&off.account).is_none());
    assert!(!upload_dir(&off).exists());

    // On: the upload dataset is the Photos app's, the Drive mirror still the Files app's.
    let mut on = rig(PhotosSwitch::On).await;
    let photos = client_of(&on, PHOTOS_APP).await;
    let files = client_of(&on, FILES_APP).await;
    on.supervisor.tick().await;
    assert_eq!(
        names(&SyncProxy::new(&photos).await.expect("proxy")).await,
        [UPLOAD_DATASET]
    );
    assert_eq!(
        names(&SyncProxy::new(&files).await.expect("proxy")).await,
        [FILES_DATASET]
    );
    assert!(on.supervisor.google_picker(&on.account).is_some());
    assert!(
        upload_dir(&on).is_dir(),
        "the folder to drop photos into exists"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_file_uploads_once_a_duplicate_does_not_and_the_album_is_the_apps_own() {
    let mut rig = rig(PhotosSwitch::On).await;
    rig.supervisor.tick().await;
    std::fs::write(upload_dir(&rig).join("a.jpg"), photo(1)).expect("photo");
    eventually("the photo is in Google Photos", || {
        rig.google.photos_items().len() == 1 && synced(&rig) == ["a.jpg"]
    })
    .await;

    let items = rig.google.photos_items();
    assert_eq!(
        (items[0].filename.as_str(), &items[0].bytes),
        ("a.jpg", &photo(1))
    );
    let albums = rig.google.photos_albums();
    assert_eq!(albums.len(), 1, "one album, made by the app");
    assert_eq!(albums[0].title, ALBUM_TITLE);
    assert_eq!(items[0].album.as_deref(), Some(albums[0].id.as_str()));

    // The same bytes under another name, and a different photo: only the second is sent.
    std::fs::write(upload_dir(&rig).join("copy.jpg"), photo(1)).expect("copy");
    std::fs::create_dir_all(upload_dir(&rig).join("trip")).expect("dir");
    std::fs::write(upload_dir(&rig).join("trip/b.jpg"), photo(2)).expect("photo");
    eventually("the new photo is up and the copy is settled", || {
        synced(&rig) == ["a.jpg", "copy.jpg", "trip/b.jpg"]
    })
    .await;
    let names: Vec<String> = rig
        .google
        .photos_items()
        .into_iter()
        .map(|i| i.filename)
        .collect();
    assert_eq!(names, ["a.jpg", "b.jpg"], "the duplicate was not sent");
    assert_eq!(count(&rig, "POST", "/v1/uploads"), 2);
    assert_eq!(rig.google.photos_albums().len(), 1);

    // A restart keeps the ledger: the same content again is still not sent.
    rig.files_shown.store(false, Ordering::SeqCst);
    rig.photos_shown.store(false, Ordering::SeqCst);
    rig.supervisor.tick().await;
    rig.supervisor = rig.new_supervisor();
    rig.files_shown.store(true, Ordering::SeqCst);
    rig.photos_shown.store(true, Ordering::SeqCst);
    rig.supervisor.tick().await;
    std::fs::write(upload_dir(&rig).join("again.jpg"), photo(2)).expect("again");
    eventually("the copy after the restart is settled", || {
        synced(&rig).contains(&"again.jpg".to_owned())
    })
    .await;
    assert_eq!(count(&rig, "POST", "/v1/uploads"), 2, "still two uploads");
    assert_eq!(rig.google.photos_items().len(), 2);

    // A local delete is not a delete in Google Photos, and nothing is ever read from it.
    std::fs::remove_file(upload_dir(&rig).join("a.jpg")).expect("delete");
    eventually("the journal forgot the deleted file", || {
        !synced(&rig).contains(&"a.jpg".to_owned())
    })
    .await;
    assert_eq!(rig.google.photos_items().len(), 2);
    let hits = rig.google.hits();
    let library: Vec<_> = hits
        .iter()
        .filter(|h| {
            h.target.starts_with("/v1/uploads")
                || h.target.starts_with("/v1/albums")
                || h.target.starts_with("/v1/mediaItems")
        })
        .collect();
    assert!(library.iter().all(|h| h.method == "POST"), "{library:?}");
    let want = format!("Bearer {UPLOAD_BEARER}");
    assert!(
        library
            .iter()
            .all(|h| h.authorization.as_deref() == Some(want.as_str()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_picker_starts_a_session_polls_it_imports_what_was_picked_and_deletes_it() {
    let mut rig = rig(PhotosSwitch::On).await;
    rig.supervisor.tick().await;
    let picker = rig
        .supervisor
        .google_picker(&rig.account)
        .expect("a picker");

    let session = picker.start().await.expect("session");
    assert!(
        session.picker_uri.contains(session.id.as_str()),
        "{session:?}"
    );
    assert_eq!(session.poll_every, std::time::Duration::from_secs(1));
    assert_eq!(picker.poll(&session.id).await, Ok(PickerState::Waiting));
    assert_eq!(picker.import(&session.id).await, Err(PickerError::NotYet));
    assert!(
        !rig.paths.photos_picked_dir(&rig.account).exists(),
        "nothing is touched before the person is done"
    );

    // The person picks in the browser: two photos (one named to climb out) and a video.
    assert!(rig.google.picker_pick(
        session.id.as_str(),
        vec![
            Pick {
                filename: "IMG_1.jpg".into(),
                bytes: photo(1),
                mime: "image/jpeg".into()
            },
            Pick {
                filename: "../evil.jpg".into(),
                bytes: photo(2),
                mime: "image/jpeg".into()
            },
            Pick {
                filename: "IMG_1.jpg".into(),
                bytes: photo(3),
                mime: "image/jpeg".into()
            },
            Pick {
                filename: "clip.mp4".into(),
                bytes: vec![9; 3000],
                mime: "video/mp4".into()
            },
        ],
    ));
    assert_eq!(picker.poll(&session.id).await, Ok(PickerState::Picked));
    let imported = picker.import(&session.id).await.expect("import");

    let dir = rig
        .paths
        .photos_picked_dir(&rig.account)
        .join(session.id.as_str());
    assert_eq!(imported.dir, dir);
    assert_eq!(imported.session, SessionEnd::Deleted);
    let got: Vec<(String, Vec<u8>)> = imported
        .files
        .iter()
        .map(|f| {
            assert_eq!(f.parent(), Some(dir.as_path()), "inside the session folder");
            (
                f.file_name().expect("name").to_string_lossy().into_owned(),
                std::fs::read(f).expect("file"),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("IMG_1.jpg".to_owned(), photo(1)),
            ("evil.jpg".to_owned(), photo(2)),
            ("IMG_1-2.jpg".to_owned(), photo(3)),
            ("clip.mp4".to_owned(), vec![9; 3000]),
        ]
    );
    assert!(
        !rig.paths
            .photos_picked_dir(&rig.account)
            .join("evil.jpg")
            .exists()
    );

    // The session is gone at Google, and a second import has nothing to ask.
    let sessions = rig.google.picker_sessions();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].deleted);
    assert_eq!(
        picker.poll(&session.id).await,
        Err(PickerError::Refused(404))
    );

    // Photos were fetched as bytes (`=d`), the video as video bytes (`=dv`), with the picker's
    // own bearer.
    assert_eq!(count(&rig, "GET", "=d"), 4);
    assert_eq!(count(&rig, "GET", "=dv"), 1);
    let want = format!("Bearer {PICKER_BEARER}");
    for hit in rig.google.hits().iter().filter(|h| {
        h.target.starts_with("/v1/sessions")
            || h.target.starts_with("/dl/")
            || h.target.starts_with("/v1/mediaItems?")
    }) {
        assert_eq!(hit.authorization.as_deref(), Some(want.as_str()), "{hit:?}");
    }

    // A session id that would climb out of the folder is never made into a path.
    assert!(SessionId::parse("../x").is_none());
}

/// The person picks one photo and one video in `session`.
fn pick_two(rig: &Rig, session: &str) {
    assert!(rig.google.picker_pick(
        session,
        vec![
            Pick {
                filename: "IMG_1.jpg".into(),
                bytes: photo(1),
                mime: "image/jpeg".into()
            },
            Pick {
                filename: "clip.mp4".into(),
                bytes: vec![9; 3000],
                mime: "video/mp4".into()
            },
        ],
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn picked_bytes_on_a_second_origin_come_with_the_bearer_when_the_provider_file_lists_it() {
    let mut rig = rig_with(PhotosSwitch::On, Media::Listed).await;
    rig.supervisor.tick().await;
    let picker = rig.supervisor.google_picker(&rig.account).expect("picker");
    let session = picker.start().await.expect("session");
    pick_two(&rig, session.id.as_str());
    let imported = picker.import(&session.id).await.expect("import");
    let got: Vec<Vec<u8>> = imported
        .files
        .iter()
        .map(|f| std::fs::read(f).expect("file"))
        .collect();
    assert_eq!(got, [photo(1), vec![9; 3000]]);

    // The bytes came from the second origin, each request with the picker audience's bearer,
    // and never from the Picker's own.
    let media = rig.media.as_ref().expect("media origin");
    let hits = media.hits();
    assert_eq!(hits.len(), 2, "{hits:?}");
    let want = format!("Bearer {PICKER_BEARER}");
    assert!(
        hits.iter()
            .all(|h| h.status == 200 && h.authorization.as_deref() == Some(want.as_str()))
    );
    assert_eq!(count(&rig, "GET", "/dl/"), 0);
    // No linked (credential-free) relay was used for them, and the secret is nowhere.
    let printed = format!("{hits:?} {:?}", rig.google.hits());
    assert!(!printed.contains(SECRET_ACCESS) && !printed.contains(SECRET_REFRESH));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_who_granted_only_the_picker_gets_the_picker_and_no_upload_dataset() {
    let mut rig = rig_picker_only(PhotosSwitch::On).await;
    rig.supervisor.tick().await;
    // No upload dataset, no upload folder, no call to the upload API (no 403 later).
    let photos = client_of(&rig, PHOTOS_APP).await;
    assert_eq!(
        names(&SyncProxy::new(&photos).await.expect("proxy")).await,
        Vec::<String>::new()
    );
    assert!(!upload_dir(&rig).exists());
    // The picker stands alone and imports.
    let picker = rig.supervisor.google_picker(&rig.account).expect("picker");
    let session = picker.start().await.expect("session");
    pick_two(&rig, session.id.as_str());
    let imported = picker.import(&session.id).await.expect("import");
    assert_eq!(imported.files.len(), 2);
    assert_eq!(count(&rig, "POST", "/v1/uploads"), 0);
    assert_eq!(count(&rig, "POST", "/v1/albums"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn picked_bytes_on_an_origin_the_provider_file_does_not_list_are_refused_and_get_no_bearer() {
    let mut rig = rig_with(PhotosSwitch::On, Media::Unlisted).await;
    rig.supervisor.tick().await;
    let picker = rig.supervisor.google_picker(&rig.account).expect("picker");
    let session = picker.start().await.expect("session");
    pick_two(&rig, session.id.as_str());
    let refused = picker.import(&session.id).await.expect_err("refused");
    assert_eq!(refused, PickerError::Unreached);
    // Nothing reached the unlisted origin; nothing of the person's library was written.
    assert!(rig.media.as_ref().expect("media origin").hits().is_empty());
    let dir = rig
        .paths
        .photos_picked_dir(&rig.account)
        .join(session.id.as_str());
    let kept = std::fs::read_dir(&dir).map_or(0, |entries| entries.count());
    assert_eq!(kept, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_google_photos_follow_the_photos_grant_and_leave_what_is_on_disk() {
    let mut rig = rig(PhotosSwitch::On).await;
    let photos = client_of(&rig, PHOTOS_APP).await;
    let sync = SyncProxy::new(&photos).await.expect("proxy");
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, [UPLOAD_DATASET]);
    std::fs::write(upload_dir(&rig).join("a.jpg"), photo(1)).expect("photo");
    eventually("it is up", || rig.google.photos_items().len() == 1).await;

    rig.photos_shown.store(false, Ordering::SeqCst);
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, Vec::<String>::new());
    assert!(rig.supervisor.google_picker(&rig.account).is_none());
    assert!(
        upload_dir(&rig).join("a.jpg").exists(),
        "the files are the person's"
    );

    // While the grant is off a new file waits; with it back, it goes.
    std::fs::write(upload_dir(&rig).join("b.jpg"), photo(2)).expect("photo");
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    assert_eq!(rig.google.photos_items().len(), 1);
    rig.photos_shown.store(true, Ordering::SeqCst);
    rig.supervisor.tick().await;
    assert_eq!(names(&sync).await, [UPLOAD_DATASET]);
    eventually("the waiting photo goes up", || {
        rig.google.photos_items().len() == 2
    })
    .await;

    // Removing the account wipes the folder, the picked photos and the ledger.
    let ledger = rig.paths.photos_ledger(&rig.account);
    assert!(ledger.exists());
    rig.photos_shown.store(false, Ordering::SeqCst);
    rig.files_shown.store(false, Ordering::SeqCst);
    syncd::removal::wipe(&rig.paths, &rig.hub, &rig.account)
        .await
        .expect("wipe");
    rig.supervisor.tick().await;
    assert!(!rig.paths.photos_dir(&rig.account).exists());
    assert!(!ledger.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn no_secret_is_in_the_ledger_the_journal_or_the_picked_files() {
    let mut rig = rig(PhotosSwitch::On).await;
    rig.supervisor.tick().await;
    std::fs::write(upload_dir(&rig).join("a.jpg"), photo(1)).expect("photo");
    eventually("it is up", || synced(&rig) == ["a.jpg"]).await;
    let picker = rig.supervisor.google_picker(&rig.account).expect("picker");
    let session = picker.start().await.expect("session");
    rig.google.picker_pick(
        session.id.as_str(),
        vec![Pick {
            filename: "p.jpg".into(),
            bytes: photo(5),
            mime: "image/jpeg".into(),
        }],
    );
    picker.import(&session.id).await.expect("import");

    let mut kept = Vec::new();
    for dir in [
        rig.paths.journals.clone(),
        rig.paths.photos_dir(&rig.account),
    ] {
        collect(&dir, &mut kept);
    }
    assert!(
        kept.iter()
            .any(|(p, _)| p.ends_with("google_photos_upload.ledger.json"))
    );
    for secret in [SECRET_ACCESS, SECRET_REFRESH, UPLOAD_BEARER, PICKER_BEARER] {
        assert!(
            !kept
                .iter()
                .any(|(_, bytes)| bytes.windows(secret.len()).any(|w| w == secret.as_bytes())),
            "{secret} is in a file syncd keeps"
        );
    }
    let printed = format!("{:?} {:?}", rig.supervisor, picker);
    for secret in [SECRET_ACCESS, SECRET_REFRESH, UPLOAD_BEARER, PICKER_BEARER] {
        assert!(
            !printed.contains(secret),
            "{secret} is in syncd's own debug text"
        );
    }
}

fn collect(dir: &std::path::Path, into: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => collect(&path, into),
            Ok(kind) if kind.is_file() => {
                if let Ok(bytes) = std::fs::read(&path) {
                    into.push((path, bytes));
                }
            }
            _ => {}
        }
    }
}
