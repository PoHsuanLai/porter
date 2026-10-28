//! The drive listing's SQLite file: every entry of the drive by id, the anchor the feed stands
//! at, and the tree the replica learnt (`porter_sync::Folders`), each page written in one
//! transaction. An entry keeps its parent's id and its own name, never its path: a path is
//! built when it is read, so a folder renamed or moved re-paths everything inside it without
//! one more row written.

use crate::journal::{JournalError, migrate};
use porter_ids::{Bytes, UnixSeconds};
use porter_sync::{
    Anchor, Change, ChangePage, ContentHash, Cursor, Folders, ItemKind, ItemPath, More, RemoteId,
    RemoteItem, RemoteVersion,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// How many folders a path may have before it is given up as a loop.
const DEPTH: usize = 64;

/// Version 1: entries, and the one row of where the feed stands.
const V1: &str = "
CREATE TABLE entries (
    id          TEXT PRIMARY KEY NOT NULL,
    parent      TEXT,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    size        INTEGER NOT NULL,
    hash        TEXT,
    version     TEXT NOT NULL,
    modified    INTEGER
);
CREATE INDEX entries_by_parent ON entries (parent, name);
CREATE TABLE feed (
    id          INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    anchor      TEXT,
    folders     TEXT NOT NULL,
    listing     INTEGER NOT NULL
);
CREATE TABLE listed (
    id          TEXT PRIMARY KEY NOT NULL
);
";

const MIGRATIONS: &[&str] = &[V1];

/// One entry of the drive: a file, a folder or a document that opens online.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveEntry {
    /// The drive's id for it.
    pub id: RemoteId,
    /// The folder it is in; none at the top of the drive.
    pub parent: Option<RemoteId>,
    /// Its name in that folder.
    pub name: String,
    /// Where it is below the top of the drive (`Docs/Plan.docx`), built from its folders.
    pub path: ItemPath,
    /// What it is.
    pub kind: ItemKind,
    /// Its size (a folder's is 0).
    pub size: Bytes,
    /// Its content hash, when the drive reports one.
    pub hash: Option<ContentHash>,
    /// The drive's version of it.
    pub version: RemoteVersion,
    /// When it last changed, when the drive says.
    pub modified: Option<UnixSeconds>,
}

/// What one page did to the entries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Applied {
    /// Entries written (new or changed).
    pub upserted: usize,
    /// Entries removed (a tombstone, or left out of a full listing).
    pub removed: usize,
}

/// The stored row of an entry, before its path is built.
struct Row {
    id: RemoteId,
    parent: Option<RemoteId>,
    name: String,
    kind: ItemKind,
    size: Bytes,
    hash: Option<ContentHash>,
    version: RemoteVersion,
    modified: Option<UnixSeconds>,
}

const COLUMNS: &str = "id, parent, name, kind, size, hash, version, modified";

/// The listing journal of one account's drive, open.
#[derive(Debug)]
pub struct DriveJournal {
    connection: Mutex<Connection>,
}

impl DriveJournal {
    /// Opens the file at `path` (`Paths::journal(account, "storage_drive")`), creating it and its
    /// directory, and migrating it.
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA synchronous = FULL;")?;
        migrate(&mut connection, MIGRATIONS)?;
        connection.execute(
            "INSERT OR IGNORE INTO feed (id, anchor, folders, listing) VALUES (1, NULL, ?1, 0)",
            [serde_json::to_string(&Folders::new()).map_err(corrupt)?],
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn connection(&self) -> MutexGuard<'_, Connection> {
        // A panic mid-statement leaves SQLite's own transaction to roll back.
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Where the next page is read from: the stored anchor, else the start (a first listing).
    pub fn cursor(&self) -> Result<Cursor, JournalError> {
        let anchor: Option<String> =
            self.connection()
                .query_row("SELECT anchor FROM feed WHERE id = 1", [], |row| row.get(0))?;
        Ok(anchor.map_or(Cursor::Start, |a| Cursor::At(Anchor(a))))
    }

    /// The tree the replica had learnt when the last page was applied, for the next replica to
    /// start from (`with_folders`).
    pub fn folders(&self) -> Result<Folders, JournalError> {
        let text: String =
            self.connection()
                .query_row("SELECT folders FROM feed WHERE id = 1", [], |row| {
                    row.get(0)
                })?;
        serde_json::from_str(&text).map_err(corrupt)
    }

    /// Whether a full listing is under way (a first one, or one after the anchor expired):
    /// its pages are noted, and what it never lists is removed when it ends.
    pub fn listing(&self) -> Result<bool, JournalError> {
        let listing: i64 =
            self.connection()
                .query_row("SELECT listing FROM feed WHERE id = 1", [], |row| {
                    row.get(0)
                })?;
        Ok(listing != 0)
    }

    /// Begins a full listing: the anchor is forgotten (it expired, or there was none), and so
    /// is the tree, which the listing learns again. The entries stay until it ends.
    pub fn begin_listing(&self) -> Result<(), JournalError> {
        let mut connection = self.connection();
        let tx = connection.transaction()?;
        tx.execute("DELETE FROM listed", [])?;
        tx.execute(
            "UPDATE feed SET anchor = NULL, folders = ?1, listing = 1 WHERE id = 1",
            [serde_json::to_string(&Folders::new()).map_err(corrupt)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Applies one page, in one transaction with the anchor it ends in and the tree the replica
    /// holds after it (`folders`). A page of a full listing notes what it lists; its last page
    /// removes every entry the listing did not list and ends it.
    pub fn apply(&self, page: &ChangePage, folders: &Folders) -> Result<Applied, JournalError> {
        let mut connection = self.connection();
        let tx = connection.transaction()?;
        let listing: i64 = tx.query_row("SELECT listing FROM feed WHERE id = 1", [], |row| {
            row.get(0)
        })?;
        let mut applied = Applied::default();
        for change in &page.changes {
            match change {
                Change::Upsert(item) => {
                    put(&tx, item, folders)?;
                    if listing != 0 {
                        tx.execute(
                            "INSERT OR IGNORE INTO listed (id) VALUES (?1)",
                            [&item.id.0],
                        )?;
                    }
                    applied.upserted += 1;
                }
                Change::Tombstone(gone) => {
                    applied.removed +=
                        tx.execute("DELETE FROM entries WHERE id = ?1", [&gone.id.0])?;
                    tx.execute("DELETE FROM listed WHERE id = ?1", [&gone.id.0])?;
                }
            }
        }
        let ended = listing != 0 && page.more == More::Done;
        if ended {
            applied.removed += tx.execute(
                "DELETE FROM entries WHERE id NOT IN (SELECT id FROM listed)",
                [],
            )?;
            tx.execute("DELETE FROM listed", [])?;
        }
        tx.execute(
            "UPDATE feed SET anchor = ?1, folders = ?2, listing = ?3 WHERE id = 1",
            params![
                page.next.0,
                serde_json::to_string(folders).map_err(corrupt)?,
                i64::from(listing != 0 && !ended),
            ],
        )?;
        tx.commit()?;
        Ok(applied)
    }

    /// The entry the drive knows as `id`.
    pub fn entry(&self, id: &RemoteId) -> Result<Option<DriveEntry>, JournalError> {
        let connection = self.connection();
        let Some(row) = row_where(&connection, "id = ?1", params![id.0])? else {
            return Ok(None);
        };
        let path = path_of(&connection, &row)?;
        Ok(path.map(|path| entry(row, path)))
    }

    /// The entry at `path` below the top of the drive; none for the top itself (`""`), which is
    /// no entry. Where a folder holds two of the same name (Drive allows it), the first by id.
    pub fn entry_at(&self, path: &ItemPath) -> Result<Option<DriveEntry>, JournalError> {
        let connection = self.connection();
        let names: Vec<&str> = path.0.split('/').filter(|n| !n.is_empty()).collect();
        let mut found: Option<Row> = None;
        for name in &names {
            found = match &found {
                None => row_where(&connection, &format!("{TOP} AND name = ?1"), params![name])?,
                Some(folder) => row_where(
                    &connection,
                    "parent = ?1 AND name = ?2",
                    params![folder.id.0, name],
                )?,
            };
            if found.is_none() {
                return Ok(None);
            }
        }
        Ok(found.map(|row| entry(row, ItemPath(names.join("/")))))
    }

    /// The entries directly inside the folder at `path` (`""` for the top of the drive), by
    /// name; none when nothing is there or it is not a folder.
    pub fn children(&self, path: &ItemPath) -> Result<Vec<DriveEntry>, JournalError> {
        let top = path.0.trim_matches('/').is_empty();
        let rows = match top {
            true => rows_where(&self.connection(), TOP, params![])?,
            false => match self.entry_at(path)? {
                Some(folder) if folder.kind == ItemKind::Folder => {
                    rows_where(&self.connection(), "parent = ?1", params![folder.id.0])?
                }
                _ => Vec::new(),
            },
        };
        let base = path.0.trim_matches('/');
        Ok(rows
            .into_iter()
            .map(|row| {
                let path = match base {
                    "" => ItemPath(row.name.clone()),
                    base => ItemPath(format!("{base}/{}", row.name)),
                };
                entry(row, path)
            })
            .collect())
    }

    /// How many entries the drive has.
    pub fn len(&self) -> Result<usize, JournalError> {
        let count: i64 =
            self.connection()
                .query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Whether the drive has no entries (nothing listed yet, or an empty drive).
    pub fn is_empty(&self) -> Result<bool, JournalError> {
        Ok(self.len()? == 0)
    }
}

impl porter_drive::Listing for DriveJournal {
    fn entry(
        &self,
        id: &RemoteId,
    ) -> Result<Option<porter_drive::Entry>, porter_drive::ListingError> {
        let connection = self.connection();
        listed_where(&connection, "id = ?1", params![id.0]).map(|mut rows| rows.pop())
    }

    fn children(
        &self,
        folder: Option<&RemoteId>,
    ) -> Result<Vec<porter_drive::Entry>, porter_drive::ListingError> {
        let connection = self.connection();
        match folder {
            None => listed_where(&connection, TOP, params![]),
            Some(folder) => listed_where(&connection, "parent = ?1", params![folder.0]),
        }
    }

    fn descendants(
        &self,
        folder: &RemoteId,
    ) -> Result<Vec<porter_drive::Entry>, porter_drive::ListingError> {
        let connection = self.connection();
        listed_where(
            &connection,
            &format!(
                "id IN (WITH RECURSIVE below(id, depth) AS (
                    SELECT id, 1 FROM entries WHERE parent = ?1
                    UNION SELECT e.id, b.depth + 1 FROM entries e JOIN below b ON e.parent = b.id
                    WHERE b.depth < {DEPTH})
                 SELECT id FROM below)"
            ),
            params![folder.0],
        )
    }
}

/// The entries `filter` picks, as the files-on-demand core reads them: a parent that is no
/// entry (the drive's root) is the top.
fn listed_where(
    connection: &Connection,
    filter: &str,
    values: impl rusqlite::Params,
) -> Result<Vec<porter_drive::Entry>, porter_drive::ListingError> {
    let fault = |e: JournalError| porter_drive::ListingError(e.to_string());
    let sql = format!(
        "SELECT id, CASE WHEN parent IN (SELECT id FROM entries) THEN parent END, name, kind, \
         size, hash, version, modified FROM entries WHERE {filter}"
    );
    let run = || -> Result<Vec<Row>, JournalError> {
        let mut statement = connection.prepare(&sql)?;
        let raw = statement
            .query_map(values, raw_row)?
            .collect::<Result<Vec<_>, _>>()?;
        raw.into_iter().map(read_row).collect()
    };
    Ok(run()
        .map_err(fault)?
        .into_iter()
        .map(|row| porter_drive::Entry {
            id: row.id,
            parent: row.parent,
            name: row.name,
            kind: row.kind,
            size: row.size,
            version: row.version,
            hash: row.hash,
            modified: row.modified,
        })
        .collect())
}

/// The rows at the top of the drive: their folder is no entry (the drive's root, which the feed
/// never reports as an item).
const TOP: &str = "(parent IS NULL OR parent NOT IN (SELECT id FROM entries))";

/// Writes `item` as its row: its parent from the tree, its name from the tree for a folder and
/// from its path below its parent's for anything else (a name may hold a `/` on Drive).
fn put(tx: &Transaction<'_>, item: &RemoteItem, folders: &Folders) -> Result<(), JournalError> {
    let parent = folders.parent_of(&item.id).cloned();
    let name = match folders.folder(&item.id) {
        Some(folder) => folder.name.clone(),
        None => name_below(&item.path, parent.as_ref(), folders),
    };
    tx.execute(
        &format!(
            "INSERT OR REPLACE INTO entries ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
        ),
        params![
            item.id.0,
            parent.as_ref().map(|p| &p.0),
            name,
            serde_json::to_string(&item.kind).map_err(corrupt)?,
            i64::try_from(item.size.0).unwrap_or(i64::MAX),
            item.hash.as_ref().map(|h| &h.0),
            item.version.0,
            item.modified.map(|m| m.0),
        ],
    )?;
    Ok(())
}

/// The last part of `path` after the path of `parent` as the tree builds it.
fn name_below(path: &ItemPath, parent: Option<&RemoteId>, folders: &Folders) -> String {
    let mut names = Vec::new();
    let mut at = parent;
    for _ in 0..DEPTH {
        match at.and_then(|id| folders.folder(id).map(|f| (id, f))) {
            Some((_, folder)) => {
                names.push(folder.name.as_str());
                at = folder.parent.as_ref();
            }
            None => break,
        }
    }
    // The drive's root is a folder of the tree too, with no parent; it is not in the path.
    if at.is_none() {
        names.pop();
    }
    names.reverse();
    let above = names.join("/");
    match above.is_empty() {
        true => path.0.clone(),
        false => path
            .0
            .strip_prefix(&format!("{above}/"))
            .map_or_else(|| last_part(&path.0), str::to_owned),
    }
}

fn last_part(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

/// `row`'s path, built up its folders to the first that is no entry (the drive's root, or one
/// a later page brings); `None` when the chain runs past [`DEPTH`] (a loop).
fn path_of(connection: &Connection, row: &Row) -> Result<Option<ItemPath>, JournalError> {
    let mut names = vec![row.name.clone()];
    let mut at = row.parent.clone();
    for _ in 0..DEPTH {
        let above = match &at {
            Some(parent) => row_where(connection, "id = ?1", params![parent.0])?,
            None => None,
        };
        // No folder above, or one that is no entry (the drive's root): the top.
        let Some(folder) = above else {
            names.reverse();
            return Ok(Some(ItemPath(names.join("/"))));
        };
        names.push(folder.name);
        at = folder.parent;
    }
    Ok(None)
}

fn row_where(
    connection: &Connection,
    filter: &str,
    values: impl rusqlite::Params,
) -> Result<Option<Row>, JournalError> {
    let sql = format!("SELECT {COLUMNS} FROM entries WHERE {filter} ORDER BY id LIMIT 1");
    let raw = connection.query_row(&sql, values, raw_row).optional()?;
    raw.map(read_row).transpose()
}

fn rows_where(
    connection: &Connection,
    filter: &str,
    values: impl rusqlite::Params,
) -> Result<Vec<Row>, JournalError> {
    let sql = format!("SELECT {COLUMNS} FROM entries WHERE {filter} ORDER BY name, id");
    let mut statement = connection.prepare(&sql)?;
    let raw = statement
        .query_map(values, raw_row)?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(read_row).collect()
}

type Raw = (
    String,
    Option<String>,
    String,
    String,
    i64,
    Option<String>,
    String,
    Option<i64>,
);

fn raw_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Raw> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn read_row(
    (id, parent, name, kind, size, hash, version, modified): Raw,
) -> Result<Row, JournalError> {
    Ok(Row {
        id: RemoteId(id),
        parent: parent.map(RemoteId),
        name,
        kind: serde_json::from_str(&kind).map_err(corrupt)?,
        size: Bytes(u64::try_from(size).unwrap_or(0)),
        hash: hash.map(ContentHash),
        version: RemoteVersion(version),
        modified: modified.map(UnixSeconds),
    })
}

fn entry(row: Row, path: ItemPath) -> DriveEntry {
    DriveEntry {
        id: row.id,
        parent: row.parent,
        name: row.name,
        path,
        kind: row.kind,
        size: row.size,
        hash: row.hash,
        version: row.version,
        modified: row.modified,
    }
}

fn corrupt(error: serde_json::Error) -> JournalError {
    JournalError::Corrupt(error.to_string())
}
