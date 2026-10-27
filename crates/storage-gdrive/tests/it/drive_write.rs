//! `GdriveReplica`'s whole-drive writes in My Drive against the fake Google: a folder, a file
//! into it by the folder's id, new content and a rename or move that keep the id, a stale base
//! refused as a conflict, the trash (never `files.delete`), and a large file sent in chunks whose
//! session survives a cut and is resumed by a new replica.

use crate::common;

use common::{Replica, Server};
use porter_sync::{
    BaseVersion, Content, DriveWrite, Place, Progress, PutRefused, RemoteSide, Session, Unheard,
    Upload, UploadTo, Written,
};
use std::sync::Mutex;
use storage_gdrive::{CHUNK_UNIT, Root, Uploads};
use test_kit::Scratch;

fn drive(server: &Server, uploads: Uploads) -> Replica {
    server.replica_at(Root::Drive, "", 50, uploads)
}

fn content(dir: &Scratch, name: &str, bytes: &[u8]) -> Content {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write");
    Content::new(path, bytes.len() as u64)
}

fn at(written: &Written) -> BaseVersion {
    BaseVersion::At(written.version.clone())
}

fn place(parent: Option<&Written>, name: &str) -> Place {
    Place::new(parent.map(|w| w.id.clone()), name)
}

async fn send(
    replica: &Replica,
    to: UploadTo,
    base: BaseVersion,
    content: &Content,
) -> Result<Written, PutRefused> {
    replica
        .upload(Upload {
            to,
            base,
            content,
            resume: None,
            progress: &Unheard,
        })
        .await
}

/// Changes the file on the drive once an upload has sent its first chunk, as another device
/// would meanwhile; keeps the session it is told of.
struct Changing {
    session: Mutex<Option<Session>>,
    change: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Changing {
    fn new(change: impl FnOnce() + Send + 'static) -> Self {
        Self {
            session: Mutex::default(),
            change: Mutex::new(Some(Box::new(change))),
        }
    }
}

impl Progress for Changing {
    fn session(&self, session: Option<&Session>) {
        *self.session.lock().expect("lock") = session.cloned();
    }

    fn sent(&self, bytes: u64) {
        let change = match bytes {
            0 => None,
            _ => self.change.lock().expect("lock").take(),
        };
        if let Some(change) = change {
            change();
        }
    }
}

/// What an upload told: the session it keeps, and the bytes sent.
#[derive(Debug, Default)]
struct Kept {
    session: Mutex<Option<Session>>,
    sent: Mutex<u64>,
}

impl Progress for Kept {
    fn session(&self, session: Option<&Session>) {
        *self.session.lock().expect("lock") = session.cloned();
    }

    fn sent(&self, bytes: u64) {
        *self.sent.lock().expect("lock") = bytes;
    }
}

impl Kept {
    fn session(&self) -> Option<Session> {
        self.session.lock().expect("lock").clone()
    }

    fn sent(&self) -> u64 {
        *self.sent.lock().expect("lock")
    }
}

#[tokio::test]
async fn folders_files_renames_and_the_trash_keep_ids_and_check_their_base() {
    let server = Server::start().await;
    let dir = Scratch::new("gdrive-drive-write");
    let replica = drive(&server, Uploads::default());
    let work = replica
        .make_folder(&place(None, "Work"))
        .await
        .expect("a folder");
    let file = send(
        &replica,
        UploadTo::New(place(Some(&work), "a.txt")),
        BaseVersion::Absent,
        &content(&dir, "a", b"one"),
    )
    .await
    .expect("a new file");
    assert_eq!(
        server.google.my_drive_file("Work/a.txt"),
        Some(b"one".to_vec())
    );

    let again = send(
        &replica,
        UploadTo::Replace(file.id.clone()),
        at(&file),
        &content(&dir, "b", b"two"),
    )
    .await
    .expect("new content");
    assert_eq!(again.id, file.id, "the same file, at a new version");
    assert_ne!(again.version, file.version);
    let stale = send(
        &replica,
        UploadTo::Replace(file.id.clone()),
        at(&file),
        &content(&dir, "c", b"three"),
    )
    .await;
    let Err(PutRefused::Conflict(conflict)) = &stale else {
        panic!("{stale:?}")
    };
    assert_eq!(conflict.remote, RemoteSide::Changed(again.version.clone()));

    let moved = replica
        .relocate(&file.id, &at(&again), &place(None, "b.txt"))
        .await
        .expect("moved and renamed");
    assert_eq!(moved.id, file.id);
    assert_eq!(server.google.my_drive_file("b.txt"), Some(b"two".to_vec()));
    assert_eq!(server.google.my_drive_file("Work/a.txt"), None);
    let stale = replica
        .relocate(&file.id, &at(&again), &place(None, "c.txt"))
        .await;
    assert!(matches!(stale, Err(PutRefused::Conflict(_))), "{stale:?}");
    assert!(replica.accepts_name("a:b?") && !replica.accepts_name(""));

    let stale = replica.trash(&file.id, &at(&again)).await;
    assert!(matches!(stale, Err(PutRefused::Conflict(_))), "{stale:?}");
    replica
        .trash(&file.id, &at(&moved))
        .await
        .expect("to the trash");
    assert_eq!(server.google.drive_trashed(), ["b.txt"]);
    assert_eq!(server.google.my_drive_file("b.txt"), None);
    let deletes = server
        .google
        .hits()
        .iter()
        .filter(|h| h.method == "DELETE")
        .count();
    assert_eq!(deletes, 0, "never files.delete");
}

#[tokio::test]
async fn a_large_file_goes_in_chunks_and_a_cut_session_is_resumed_by_a_new_replica() {
    let server = Server::start().await;
    let dir = Scratch::new("gdrive-drive-resume");
    let bytes: Vec<u8> = (0..CHUNK_UNIT * 3 + 10).map(|i| (i % 251) as u8).collect();
    let big = content(&dir, "big", &bytes);
    // Every file in a session, one unit a chunk.
    let uploads = Uploads::new(0, 1);
    server.google.drive_cut_upload_after(1);
    let kept = Kept::default();
    let upload = |resume| Upload {
        to: UploadTo::New(place(None, "big.bin")),
        base: BaseVersion::Absent,
        content: &big,
        resume,
        progress: &kept,
    };
    let cut = drive(&server, uploads).upload(upload(None)).await;
    assert!(matches!(cut, Err(PutRefused::Transient(_))), "{cut:?}");
    let session = kept.session().expect("the session is kept for a resume");
    assert_eq!(kept.sent(), CHUNK_UNIT as u64);
    assert_eq!(server.google.my_drive_file("big.bin"), None);

    drive(&server, uploads)
        .upload(upload(Some(session)))
        .await
        .expect("resumed");
    assert_eq!(server.google.my_drive_file("big.bin"), Some(bytes));
    assert_eq!(kept.session(), None, "a finished session is forgotten");
    let chunks = server
        .google
        .hits()
        .iter()
        .filter(|h| h.method == "PUT")
        .count();
    assert_eq!(
        chunks, 6,
        "one sent, one cut, the question where it stands, and the three after it"
    );
}

#[tokio::test]
async fn an_item_made_under_a_reserved_id_is_found_again_and_never_made_twice() {
    let server = Server::start().await;
    let dir = Scratch::new("gdrive-drive-reserved");
    let replica = drive(&server, Uploads::default());
    let ids = replica.reserve_ids(2).await.expect("ids");
    assert_eq!(ids.len(), 2);
    let mut folder = place(None, "Work");
    folder.id = Some(ids[0].clone());
    let made = replica.make_folder(&folder).await.expect("a folder");
    assert_eq!(made.id, ids[0]);
    let again = replica.make_folder(&folder).await;
    let Err(PutRefused::Conflict(conflict)) = &again else {
        panic!("{again:?}")
    };
    assert_eq!(
        conflict.remote,
        RemoteSide::Exists(ids[0].clone(), made.version.clone())
    );
    let mut file = place(Some(&made), "a.txt");
    file.id = Some(ids[1].clone());
    for (small, uploads) in [(true, Uploads::default()), (false, Uploads::new(0, 1))] {
        let mut file = file.clone();
        if !small {
            file.id = Some(replica.reserve_ids(1).await.expect("id")[0].clone());
            file.name = "b.txt".into();
        }
        let replica = drive(&server, uploads);
        let bytes = content(&dir, &file.name, b"one");
        let sent = send(
            &replica,
            UploadTo::New(file.clone()),
            BaseVersion::Absent,
            &bytes,
        )
        .await
        .expect("a file");
        assert_eq!(Some(&sent.id), file.id.as_ref());
        let again = send(
            &replica,
            UploadTo::New(file.clone()),
            BaseVersion::Absent,
            &bytes,
        )
        .await;
        let Err(PutRefused::Conflict(conflict)) = &again else {
            panic!("{again:?}")
        };
        assert_eq!(
            conflict.remote,
            RemoteSide::Exists(sent.id.clone(), sent.version.clone()),
            "small: {small}"
        );
    }
    let found = replica.look(&ids[1]).await.expect("look").expect("found");
    assert_eq!((found.size, found.folder), (3, false));
    assert!(found.hash.is_some(), "its MD5");
    let names: Vec<String> = server
        .google
        .drive_files()
        .into_iter()
        .map(|(path, _)| path)
        .filter(|path| path.ends_with("a.txt") || path.ends_with("b.txt"))
        .collect();
    assert_eq!(names.len(), 2, "one of each: {names:?}");
}

#[tokio::test]
async fn a_session_replacing_a_file_changed_meanwhile_stops_before_its_last_chunk() {
    let server = Server::start().await;
    let dir = Scratch::new("gdrive-drive-changed");
    let uploads = Uploads::new(0, 1);
    let replica = drive(&server, uploads);
    let file = send(
        &replica,
        UploadTo::New(place(None, "big.bin")),
        BaseVersion::Absent,
        &content(&dir, "a", b"old"),
    )
    .await
    .expect("a file");
    let bytes: Vec<u8> = (0..CHUNK_UNIT * 2 + 5).map(|i| (i % 251) as u8).collect();
    let big = content(&dir, "big", &bytes);
    let google = (*server.google).clone();
    let changing = Changing::new(move || google.my_drive_put_file("big.bin", b"theirs"));
    let stale = replica
        .upload(Upload {
            to: UploadTo::Replace(file.id.clone()),
            base: at(&file),
            content: &big,
            resume: None,
            progress: &changing,
        })
        .await;
    assert!(matches!(stale, Err(PutRefused::Conflict(_))), "{stale:?}");
    assert_eq!(
        server.google.my_drive_file("big.bin"),
        Some(b"theirs".to_vec())
    );
    assert_eq!(*changing.session.lock().expect("lock"), None);
}
