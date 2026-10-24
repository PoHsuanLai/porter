//! `GraphReplica`'s whole-drive writes against the fake Graph drive: a folder, a file into it by
//! the folder's id, new content and a rename or move that keep the id, a stale base refused as a
//! conflict, the recycle bin, and a large file sent in chunks whose session survives a cut and is
//! resumed by a new replica.

use crate::common;

use common::{Replica, Server};
use porter_fake::Scratch;
use porter_sync::{
    BaseVersion, Content, DriveWrite, Place, Progress, PutRefused, RemoteSide, Session, Unheard,
    Upload, UploadTo, Written,
};
use std::sync::Mutex;
use storage_graph::{CHUNK_UNIT, Root, Uploads};

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
    Place {
        parent: parent.map(|w| w.id.clone()),
        name: name.to_owned(),
    }
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
async fn folders_files_renames_and_the_recycle_bin_keep_ids_and_check_their_base() {
    let server = Server::start().await;
    let dir = Scratch::new("graph-drive-write");
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
    assert_eq!(server.graph.root_file("Work/a.txt"), Some(b"one".to_vec()));

    let again = send(
        &replica,
        UploadTo::Replace(file.id.clone()),
        at(&file),
        &content(&dir, "b", b"two"),
    )
    .await
    .expect("new content");
    assert_eq!(again.id, file.id, "the same item, at a new version");
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
    assert_eq!(server.graph.root_file("b.txt"), Some(b"two".to_vec()));
    assert_eq!(server.graph.root_file("Work/a.txt"), None);
    let stale = replica
        .relocate(&file.id, &at(&again), &place(None, "c.txt"))
        .await;
    assert!(matches!(stale, Err(PutRefused::Conflict(_))), "{stale:?}");
    let taken = replica.make_folder(&place(None, "Work")).await;
    let Err(PutRefused::Conflict(conflict)) = &taken else {
        panic!("{taken:?}")
    };
    assert!(matches!(&conflict.remote, RemoteSide::Exists(id, _) if *id == work.id));
    assert!(!replica.accepts_name("a:b") && replica.accepts_name("a b"));

    let stale = replica.trash(&file.id, &at(&again)).await;
    assert!(matches!(stale, Err(PutRefused::Conflict(_))), "{stale:?}");
    replica
        .trash(&file.id, &at(&moved))
        .await
        .expect("to the recycle bin");
    assert_eq!(server.graph.recycle_bin(), ["b.txt"]);
    assert_eq!(server.graph.root_file("b.txt"), None);
}

#[tokio::test]
async fn a_large_file_goes_in_chunks_and_a_cut_session_is_resumed_by_a_new_replica() {
    let server = Server::start().await;
    let dir = Scratch::new("graph-drive-resume");
    let bytes: Vec<u8> = (0..CHUNK_UNIT * 3 + 10).map(|i| (i % 251) as u8).collect();
    let big = content(&dir, "big", &bytes);
    // Every file in a session, one unit a chunk.
    let uploads = Uploads::new(0, 1);
    server.graph.cut_upload_after(1);
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
    assert_eq!(server.graph.root_file("big.bin"), None);

    drive(&server, uploads)
        .upload(upload(Some(session)))
        .await
        .expect("resumed");
    assert_eq!(server.graph.root_file("big.bin"), Some(bytes));
    assert_eq!(kept.session(), None, "a finished session is forgotten");
    let chunks = server
        .graph
        .hits()
        .iter()
        .filter(|h| h.method == "PUT" && h.target.starts_with("/upload/"))
        .count();
    assert_eq!(
        chunks, 5,
        "one sent, one cut, and the three after it: none sent twice"
    );
}
