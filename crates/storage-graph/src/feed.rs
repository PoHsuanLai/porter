//! The change feed: a delta query over the dataset's folder (in a whole drive, over the drive's
//! root, the items outside the folder being left out). The first listing is the delta query
//! without a token (everything there is, no tombstones); every page ends in a link, the next
//! page's or, on the last page, the delta link, which is the anchor to resume from.
//!
//! A delta item names its parent by id and often has no path, so a path is built by walking the
//! folders seen up to the dataset's folder, asking the server for one that has not been seen.
//! A deleted folder is reported alone: the files known to have been in it are given tombstones
//! here (in a whole drive, which reports folders as items, the folders inside it and itself too).

use crate::addr::{Root, item_path};
use crate::json::{DeltaPage, DriveItem, Kind};
use crate::replica::GraphReplica;
use porter_http::http::Method;
use porter_http::http::header::CONTENT_TYPE;
use porter_http::{Http, request};
use porter_ids::{Bytes, UnixSeconds, WebUrl};
use porter_sync::DELETED;
use porter_sync::status::{read_error, unreached};
use porter_sync::{
    Anchor, Change, ChangePage, Cursor, Folder, ItemKind, ItemPath, More, RemoteId, RemoteItem,
    RemoteVersion, ReplicaError, RetryAfter, Tombstone,
};

/// How many folders a path may have above a file before the walk gives up.
const DEPTH: usize = 64;

fn busy() -> ReplicaError {
    ReplicaError::Transient(RetryAfter(30))
}

impl<H: Http> GraphReplica<H> {
    pub(crate) async fn feed(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
        let (url, resuming) = match from {
            Cursor::Start => {
                // The folder first: a dataset's folder that does not exist yet is made.
                self.root_id(true).await?;
                (self.delta_start()?, false)
            }
            Cursor::At(Anchor(link)) => match self.addr.link(&link) {
                Some(url) => (url, true),
                None => return Err(ReplicaError::AnchorExpired),
            },
        };
        // Page size is a preference of the request, not of the link: a delta link carries none.
        let get = request(Method::GET, &url)
            .header("Prefer", format!("odata.maxpagesize={}", self.page))
            .body(Vec::new())
            .map_err(|e| unreached(e.into()))?;
        let response = self.send(get).await?;
        match (response.status().as_u16(), resuming) {
            (200, _) => {}
            // `resyncRequired` and its kin: the server dropped what the link stands for.
            (410, true) => return Err(ReplicaError::AnchorExpired),
            _ => return Err(read_error(&response, self.clock.now())),
        }
        let page: DeltaPage = serde_json::from_slice(response.body()).map_err(|_| busy())?;
        let (next, more) = match (page.next, page.delta) {
            (Some(next), _) => (next, More::More),
            (None, Some(delta)) => (delta, More::Done),
            (None, None) => return Err(busy()),
        };
        let changes = self.apply(page.value).await?;
        Ok(ChangePage {
            changes,
            next: Anchor(next),
            more,
        })
    }

    fn delta_start(&self) -> Result<WebUrl, ReplicaError> {
        self.addr.delta().ok_or(ReplicaError::Gone)
    }

    /// The dataset folder's item id; with `make`, the folder is created if it is missing.
    pub(crate) async fn root_id(&self, make: bool) -> Result<String, ReplicaError> {
        if let Some(id) = self.state().root.clone() {
            return Ok(id);
        }
        let mut found = self.read_item(self.addr.path(None, None, "")).await?;
        if found.is_none() && make {
            self.make_folder().await?;
            found = self.read_item(self.addr.path(None, None, "")).await?;
        }
        let id = found.ok_or(ReplicaError::Gone)?.id;
        self.state().root = Some(id.clone());
        Ok(id)
    }

    /// Makes the dataset's folder, one level at a time; a level that exists is fine.
    async fn make_folder(&self) -> Result<(), ReplicaError> {
        let names = self.addr.folder().to_vec();
        for (at, name) in names.iter().enumerate() {
            let url = self.addr.under_root(&names[..at], Some("children"), "");
            let body = format!(
                r#"{{"name":{},"folder":{{}},"@microsoft.graph.conflictBehavior":"fail"}}"#,
                json_string(name)
            );
            let make = request(Method::POST, &url.ok_or(ReplicaError::Gone)?)
                .header(CONTENT_TYPE, "application/json")
                .body(body.into_bytes())
                .map_err(|e| unreached(e.into()))?;
            let response = self.send(make).await?;
            // 409: the level is already there.
            if !matches!(response.status().as_u16(), 200 | 201 | 409) {
                return Err(read_error(&response, self.clock.now()));
            }
        }
        Ok(())
    }

    /// Reads a delta page's items into changes, oldest first.
    async fn apply(&self, items: Vec<DriveItem>) -> Result<Vec<Change>, ReplicaError> {
        {
            // Folders first: a file's parent may come after it in the page.
            let mut state = self.state();
            for item in items.iter().filter(|i| i.kind() == Kind::Folder) {
                state
                    .folders
                    .insert_folder(RemoteId(item.id.clone()), folder_of(item));
            }
        }
        // An app folder's dataset is its files; a whole drive's is its folders too.
        let folders = self.addr.root() == Root::Drive;
        let mut changes = Vec::new();
        for item in items {
            match item.kind() {
                Kind::Deleted => changes.extend(self.gone(&item.id)),
                Kind::File => changes.extend(self.upsert(&item, ItemKind::File).await?),
                Kind::Folder if folders => {
                    changes.extend(self.upsert(&item, ItemKind::Folder).await?);
                }
                Kind::Folder | Kind::Other => {}
            }
        }
        Ok(changes)
    }

    fn tombstone(&self, id: &str) -> Change {
        Change::Tombstone(Tombstone {
            id: RemoteId(id.to_owned()),
            version: RemoteVersion(DELETED.to_owned()),
            deleted_at: self.clock.now(),
        })
    }

    /// The tombstones a deletion of `id` is: its own, and those of the files it held if it was a
    /// folder (in a whole drive, of the folders inside it and its own too, inner ones first).
    fn gone(&self, id: &str) -> Vec<Change> {
        let id = RemoteId(id.to_owned());
        let Some(held) = self.state().folders.remove(&id) else {
            return vec![self.tombstone(&id.0)];
        };
        let folders = match self.addr.root() {
            Root::Drive => held.folders.into_iter().chain([id]).collect(),
            Root::AppFolder => Vec::new(),
        };
        held.files
            .iter()
            .chain(&folders)
            .map(|gone| self.tombstone(&gone.0))
            .collect()
    }

    async fn upsert(
        &self,
        item: &DriveItem,
        kind: ItemKind,
    ) -> Result<Option<Change>, ReplicaError> {
        let (Some(parent), Some(name)) = (item.parent_id(), item.name.as_deref()) else {
            return Ok(None);
        };
        let Some(path) = self.path_of(parent, name).await? else {
            // Outside the dataset's folder.
            return Ok(None);
        };
        let file = kind.is_file();
        if file {
            self.state()
                .folders
                .insert_file(RemoteId(item.id.clone()), RemoteId(parent.to_owned()));
        }
        Ok(Some(Change::Upsert(RemoteItem {
            id: RemoteId(item.id.clone()),
            version: RemoteVersion(item.etag.clone().unwrap_or_default()),
            path,
            // A folder has no bytes of its own (Graph's size is that of what it holds).
            size: Bytes(if file { item.size.unwrap_or(0) } else { 0 }),
            hash: item.hash(),
            kind,
            modified: item.modified.as_deref().and_then(UnixSeconds::from_rfc3339),
        })))
    }

    /// The path of the file `name` in the folder `parent`, below the dataset's folder; `None`
    /// when the folder is not below it.
    pub(crate) async fn path_of(
        &self,
        parent: &str,
        name: &str,
    ) -> Result<Option<ItemPath>, ReplicaError> {
        let root = self.root_id(false).await?;
        let mut names = vec![name.to_owned()];
        let mut at = parent.to_owned();
        for _ in 0..DEPTH {
            if at == root {
                names.reverse();
                return Ok(Some(item_path(&names)));
            }
            let id = RemoteId(at.clone());
            let known = self.state().folders.folder(&id).cloned();
            let folder = match known {
                Some(known) => known,
                None => match self.item_now(&at).await? {
                    Some(item) => {
                        let known = folder_of(&item);
                        self.state().folders.insert_folder(id, known.clone());
                        known
                    }
                    None => return Ok(None),
                },
            };
            let Some(RemoteId(up)) = folder.parent else {
                return Ok(None);
            };
            names.push(folder.name);
            at = up;
        }
        Ok(None)
    }
}

/// A folder item as the tree keeps it.
pub(crate) fn folder_of(item: &DriveItem) -> Folder {
    Folder {
        name: item.name.clone().unwrap_or_default(),
        parent: item.parent_id().map(|p| RemoteId(p.to_owned())),
    }
}

/// A JSON string literal.
pub(crate) fn json_string(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}
