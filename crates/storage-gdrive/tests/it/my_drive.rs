//! `GdriveReplica` over My Drive (`Root::Drive`) against the fake Google: the listing paged and
//! resumed, folders as items, Google's own documents and shortcuts as items that open online,
//! what is not in My Drive left out, a move as a new path, and a folder trashed while no replica
//! ran, which a kept tree still knows the files of. A document exports as its file
//! (`files.export`); a Form, or one past the export limit, is refused.

use crate::common;

use common::{Replica, Server};
use porter_capability::StorageScope;
use porter_ids::WebUrl;
use porter_sync::{
    Anchor, Change, Cursor, ExportError, Folders, ItemKind, More, RemoteId, Replica as _,
    ReplicaError,
};
use storage_gdrive::{Root, Uploads};

const DOC: &str = "application/vnd.google-apps.document";
const SHEET: &str = "application/vnd.google-apps.spreadsheet";
const FORM: &str = "application/vnd.google-apps.form";
const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

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

/// The upserts' paths and kinds, in path order.
fn items(changes: &[Change]) -> Vec<(String, ItemKind)> {
    let mut found: Vec<(String, ItemKind)> = changes
        .iter()
        .filter_map(|c| match c {
            Change::Upsert(item) => Some((item.path.0.clone(), item.kind.clone())),
            Change::Tombstone(_) => None,
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
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

fn online(url: &str, export: Option<&str>) -> ItemKind {
    ItemKind::Online {
        url: WebUrl::parse(url).expect("url"),
        export: export.map(str::to_owned),
    }
}

fn my_drive(server: &Server, page: usize) -> Replica {
    server.replica_at(Root::Drive, "", page, Uploads::default())
}

/// What the planted drive holds, by name, for the assertions.
struct Planted {
    plan: String,
    budget: String,
    survey: String,
}

/// My Drive with a folder tree, files, a Doc, a Sheet, a Form and a shortcut to the Sheet;
/// beside it a file in the app data folder and one shared with the person, neither in My Drive.
fn plant(server: &Server) -> Planted {
    let google = &server.google;
    google.my_drive_put_file("Docs/a.txt", b"a");
    google.my_drive_put_file("Docs/Sub/b.txt", b"bb");
    google.my_drive_put_file("c.txt", b"ccc");
    let plan = google.my_drive_document("Docs/Plan", DOC);
    let budget = google.my_drive_document("Budget", SHEET);
    let survey = google.my_drive_document("Survey", FORM);
    google.my_drive_shortcut("Docs/Budget link", &budget);
    google.drive_put_file("app.bin", b"app");
    google.drive_shared_file("from a friend.pdf", b"pdf");
    Planted {
        plan,
        budget,
        survey,
    }
}

fn page_of(id: &str) -> String {
    format!("https://docs.example.test/d/{id}/edit")
}

#[tokio::test]
async fn my_drive_is_listed_in_pages_with_folders_documents_and_shortcuts_as_items() {
    let server = Server::start().await;
    let planted = plant(&server);
    let (changes, _, pages) = drain(&my_drive(&server, 2), Cursor::Start).await;
    assert!(pages > 1, "the listing took {pages} page(s)");
    let link = format!("https://drive.google.com/open?id={}", planted.budget);
    let want = vec![
        (
            "Budget".to_owned(),
            online(&page_of(&planted.budget), Some(XLSX)),
        ),
        ("Docs".to_owned(), ItemKind::Folder),
        ("Docs/Budget link".to_owned(), online(&link, Some(XLSX))),
        (
            "Docs/Plan".to_owned(),
            online(&page_of(&planted.plan), Some(DOCX)),
        ),
        ("Docs/Sub".to_owned(), ItemKind::Folder),
        ("Docs/Sub/b.txt".to_owned(), ItemKind::File),
        ("Docs/a.txt".to_owned(), ItemKind::File),
        ("Survey".to_owned(), online(&page_of(&planted.survey), None)),
        ("c.txt".to_owned(), ItemKind::File),
    ];
    // Neither the app data folder's file nor the one shared with the person is in My Drive.
    assert_eq!(items(&changes), want);
    let listings: Vec<String> = server
        .google
        .hits()
        .into_iter()
        .map(|h| h.target)
        .filter(|t| t.starts_with("/drive/v3/files?"))
        .collect();
    assert!(!listings.is_empty());
    for target in &listings {
        assert!(target.contains("spaces=drive&"), "{target}");
        assert!(target.contains("corpora=user"), "{target}");
        assert!(!target.contains("AllDrives"), "{target}");
    }
}

#[tokio::test]
async fn an_anchor_resumes_in_a_new_replica_and_an_expired_one_lists_again() {
    let server = Server::start().await;
    plant(&server);
    let (listed, anchor, _) = drain(&my_drive(&server, 3), Cursor::Start).await;
    server.google.my_drive_put_file("Docs/new.txt", b"n");
    server.google.drive_put_file("app2.bin", b"not in My Drive");
    let replica = my_drive(&server, 3);
    let (changes, anchor, _) = drain(&replica, Cursor::At(anchor)).await;
    assert_eq!(
        items(&changes),
        [("Docs/new.txt".to_owned(), ItemKind::File)]
    );
    let asked: Vec<String> = server.google.hits().into_iter().map(|h| h.target).collect();
    assert!(
        asked
            .iter()
            .any(|t| t.starts_with("/drive/v3/changes?") && t.contains("restrictToMyDrive=true")),
        "{asked:?}"
    );
    server.google.drive_expire_tokens();
    assert_eq!(
        replica.changes(Cursor::At(anchor)).await,
        Err(ReplicaError::AnchorExpired)
    );
    let (relisted, _, _) = drain(&replica, Cursor::Start).await;
    assert_eq!(items(&relisted).len(), items(&listed).len() + 1);
    assert!(tombstones(&relisted).is_empty());
}

#[tokio::test]
async fn a_move_is_the_same_item_at_a_new_path() {
    let server = Server::start().await;
    plant(&server);
    let replica = my_drive(&server, 50);
    let (listed, anchor, _) = drain(&replica, Cursor::Start).await;
    let a = id_at(&listed, "Docs/a.txt");
    server.google.my_drive_move("Docs/a.txt", "Archive/a.txt");
    let (changes, _, _) = drain(&replica, Cursor::At(anchor)).await;
    assert_eq!(id_at(&changes, "Archive/a.txt"), a);
    assert!(tombstones(&changes).is_empty());
    assert_eq!(
        items(&changes),
        [
            ("Archive".to_owned(), ItemKind::Folder),
            ("Archive/a.txt".to_owned(), ItemKind::File),
        ]
    );
}

#[tokio::test]
async fn a_folder_trashed_while_no_replica_ran_tombstones_what_it_held_from_the_kept_tree() {
    let server = Server::start().await;
    plant(&server);
    let first = my_drive(&server, 50);
    let (listed, anchor, _) = drain(&first, Cursor::Start).await;
    let kept: Folders = first.folders();
    // What a dataset stores is the tree's serde form.
    let stored = serde_json::to_string(&kept).expect("stored");
    drop(first);
    server.google.my_drive_trash("Docs");

    let restarted =
        my_drive(&server, 50).with_folders(serde_json::from_str(&stored).expect("read back"));
    let (changes, _, _) = drain(&restarted, Cursor::At(anchor.clone())).await;
    let mut held: Vec<RemoteId> = [
        "Docs",
        "Docs/Sub",
        "Docs/a.txt",
        "Docs/Sub/b.txt",
        "Docs/Plan",
        "Docs/Budget link",
    ]
    .iter()
    .map(|path| id_at(&listed, path))
    .collect();
    held.sort();
    assert_eq!(tombstones(&changes), held);

    // Without the tree, the feed's one trashed folder is all there is to say.
    let forgetful = my_drive(&server, 50);
    let (changes, _, _) = drain(&forgetful, Cursor::At(anchor)).await;
    assert_eq!(tombstones(&changes), [id_at(&listed, "Docs")]);
}

#[tokio::test]
async fn my_drive_reports_full_scope_and_an_app_folder_its_own() {
    let server = Server::start().await;
    assert_eq!(my_drive(&server, 50).features().scope, StorageScope::Full);
    let app = server.replica("", 50, Uploads::default());
    assert_eq!(app.features().scope, StorageScope::AppFolder);
    assert!(app.folders().is_empty());
}

#[tokio::test]
async fn a_document_exports_as_its_file_and_a_form_or_a_large_one_is_refused() {
    let server = Server::start().await;
    let planted = plant(&server);
    let replica = my_drive(&server, 100);
    let blob = replica
        .export(&RemoteId(planted.plan.clone()), DOCX)
        .await
        .expect("export");
    assert_eq!(
        blob.0,
        format!("export of {} as {DOCX}", planted.plan).into_bytes()
    );
    assert_eq!(
        replica
            .export(&RemoteId(planted.survey.clone()), DOCX)
            .await,
        Err(ExportError::Unsupported)
    );
    server.google.my_drive_export_too_large(&planted.budget);
    assert_eq!(
        replica
            .export(&RemoteId(planted.budget.clone()), XLSX)
            .await,
        Err(ExportError::TooLarge)
    );
    assert_eq!(
        replica.export(&RemoteId("nothing".into()), DOCX).await,
        Err(ExportError::Read(ReplicaError::Gone))
    );
}
