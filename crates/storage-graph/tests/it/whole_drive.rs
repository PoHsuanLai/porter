//! `GraphReplica` over the whole drive (`Root::Drive`) against the fake Graph drive: the root's
//! delta paged and resumed, folders as items, a move as a new path, and a folder deleted while
//! no replica ran, which a kept tree still knows the files of.

use crate::common;

use common::{Replica, Server};
use porter_capability::StorageScope;
use porter_sync::{
    Anchor, Change, Cursor, Folders, ItemKind, More, RemoteId, Replica as _, ReplicaError,
};
use storage_graph::{Root, Uploads};

/// Every page from `from` until the feed is caught up: the changes, the last anchor, and how
/// many pages it took.
async fn drain(replica: &Replica, from: Cursor) -> (Vec<Change>, Anchor, usize) {
    let mut changes = Vec::new();
    let mut cursor = from;
    let mut pages = 0;
    loop {
        let page = replica.changes(cursor).await.expect("page");
        pages += 1;
        changes.extend(page.changes);
        match page.more {
            More::More => cursor = Cursor::At(page.next),
            More::Done => return (changes, page.next, pages),
        }
    }
}

/// The upserts' paths and whether each is a folder, in path order.
fn items(changes: &[Change]) -> Vec<(String, bool)> {
    let mut found: Vec<(String, bool)> = changes
        .iter()
        .filter_map(|c| match c {
            Change::Upsert(item) => Some((item.path.0.clone(), item.kind == ItemKind::Folder)),
            Change::Tombstone(_) => None,
        })
        .collect();
    found.sort();
    found
}

fn tombstones(changes: &[Change]) -> Vec<RemoteId> {
    let mut found: Vec<RemoteId> = changes
        .iter()
        .filter_map(|c| match c {
            Change::Tombstone(t) => Some(t.id.clone()),
            Change::Upsert(_) => None,
        })
        .collect();
    found.sort();
    found
}

fn id_at(changes: &[Change], path: &str) -> RemoteId {
    changes
        .iter()
        .find_map(|c| match c {
            Change::Upsert(item) if item.path.0 == path => Some(item.id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{path} is listed"))
}

fn drive(server: &Server, page: usize) -> Replica {
    server.replica_at(Root::Drive, "", page, Uploads::default())
}

/// A drive with a folder tree, a file at the top and a file the app folder holds.
fn plant(server: &Server) {
    server.graph.put_root_file("Docs/a.txt", b"a");
    server.graph.put_root_file("Docs/Sub/b.txt", b"bb");
    server.graph.put_root_file("c.txt", b"ccc");
    server.graph.put_file("app.bin", b"app");
}

#[tokio::test]
async fn the_whole_drive_is_listed_in_pages_with_its_folders_as_items() {
    let server = Server::start().await;
    plant(&server);
    let (changes, _, pages) = drain(&drive(&server, 2), Cursor::Start).await;
    assert!(pages > 1, "the listing took {pages} page(s)");
    assert_eq!(
        items(&changes),
        [
            ("Apps".to_owned(), true),
            ("Apps/Quire".to_owned(), true),
            ("Apps/Quire/app.bin".to_owned(), false),
            ("Docs".to_owned(), true),
            ("Docs/Sub".to_owned(), true),
            ("Docs/Sub/b.txt".to_owned(), false),
            ("Docs/a.txt".to_owned(), false),
            ("c.txt".to_owned(), false),
        ]
    );
    let sizes: Vec<(String, u64)> = changes
        .iter()
        .filter_map(|c| match c {
            Change::Upsert(item) if item.path.0.starts_with("Docs") => {
                Some((item.path.0.clone(), item.size.0))
            }
            _ => None,
        })
        .collect();
    assert!(sizes.contains(&("Docs".to_owned(), 0)), "{sizes:?}");
    assert!(
        sizes.contains(&("Docs/Sub/b.txt".to_owned(), 2)),
        "{sizes:?}"
    );
    // The root's own delta, not a folder's.
    let asked: Vec<String> = server.graph.hits().into_iter().map(|h| h.target).collect();
    assert!(
        asked.iter().any(|p| p == "/v1.0/me/drive/root/delta"),
        "{asked:?}"
    );
    assert!(!asked.iter().any(|p| p.contains("approot")), "{asked:?}");
}

#[tokio::test]
async fn an_anchor_resumes_in_a_new_replica_and_an_expired_one_lists_again() {
    let server = Server::start().await;
    plant(&server);
    let (_, anchor, _) = drain(&drive(&server, 3), Cursor::Start).await;
    server.graph.put_root_file("Docs/new.txt", b"n");
    let replica = drive(&server, 3);
    let (changes, anchor, _) = drain(&replica, Cursor::At(anchor)).await;
    // The file, and its folder (a folder changes when a child does).
    assert_eq!(
        items(&changes),
        [
            ("Docs".to_owned(), true),
            ("Docs/new.txt".to_owned(), false)
        ]
    );
    server.graph.expire_delta_tokens();
    assert_eq!(
        replica.changes(Cursor::At(anchor)).await,
        Err(ReplicaError::AnchorExpired)
    );
    let (relisted, _, _) = drain(&replica, Cursor::Start).await;
    assert_eq!(items(&relisted).len(), 9);
    assert!(tombstones(&relisted).is_empty());
}

#[tokio::test]
async fn a_move_is_the_same_item_at_a_new_path() {
    let server = Server::start().await;
    plant(&server);
    let replica = drive(&server, 50);
    let (listed, anchor, _) = drain(&replica, Cursor::Start).await;
    let a = id_at(&listed, "Docs/a.txt");
    assert!(
        server
            .graph
            .move_root_item("Docs/a.txt", "Archive/2026/a.txt")
    );
    let (changes, _, _) = drain(&replica, Cursor::At(anchor)).await;
    assert_eq!(id_at(&changes, "Archive/2026/a.txt"), a);
    assert!(tombstones(&changes).is_empty());
    assert!(items(&changes).contains(&("Archive/2026".to_owned(), true)));
}

#[tokio::test]
async fn a_folder_deleted_while_no_replica_ran_tombstones_what_it_held_from_the_kept_tree() {
    let server = Server::start().await;
    plant(&server);
    let first = drive(&server, 50);
    let (listed, anchor, _) = drain(&first, Cursor::Start).await;
    let kept: Folders = first.folders();
    // What a dataset stores is the tree's serde form.
    let stored = serde_json::to_string(&kept).expect("stored");
    drop(first);
    server.graph.delete_root_item("Docs");

    let restarted =
        drive(&server, 50).with_folders(serde_json::from_str(&stored).expect("read back"));
    let (changes, _, _) = drain(&restarted, Cursor::At(anchor.clone())).await;
    let mut held: Vec<RemoteId> = ["Docs", "Docs/Sub", "Docs/a.txt", "Docs/Sub/b.txt"]
        .iter()
        .map(|path| id_at(&listed, path))
        .collect();
    held.sort();
    assert_eq!(tombstones(&changes), held);

    // Without the tree, the feed's one deleted folder is all there is to say.
    let forgetful = drive(&server, 50);
    let (changes, _, _) = drain(&forgetful, Cursor::At(anchor)).await;
    assert_eq!(tombstones(&changes), [id_at(&listed, "Docs")]);
}

#[tokio::test]
async fn a_whole_drive_reports_full_scope_and_an_app_folder_its_own() {
    let server = Server::start().await;
    assert_eq!(drive(&server, 50).features().scope, StorageScope::Full);
    let app = server.replica("", 50, Uploads::default());
    assert_eq!(app.features().scope, StorageScope::AppFolder);
    assert!(app.folders().is_empty());
}
