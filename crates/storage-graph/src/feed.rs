//! The change feed: a delta query over the dataset's folder. The first listing is the delta
//! query without a token (everything there is, no tombstones); every page ends in a link, the
//! next page's or, on the last page, the delta link, which is the anchor to resume from.
//!
//! A delta item names its parent by id and often has no path, so a path is built by walking the
//! folders seen up to the dataset's folder, asking the server for one that has not been seen.
//! A deleted folder is reported alone: the files known to have been in it are given tombstones
//! here.

use crate::addr::item_path;
use crate::json::{DeltaPage, DriveItem, Kind};
use crate::refuse::read_error;
use crate::replica::GraphReplica;
use porter_core::{Bytes, WebUrl};
use porter_http::{Http, HttpRequest, Method};
use porter_sync::{
    Anchor, Change, ChangePage, Cursor, ItemPath, More, RemoteId, RemoteItem, RemoteVersion,
    ReplicaError, RetryAfter, Tombstone,
};
use storage_webdav::DELETED;

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
        let request = HttpRequest::new(Method::Get, url)
            .with_header("Prefer", format!("odata.maxpagesize={}", self.page));
        let response = self.send(request).await?;
        match (response.status.0, resuming) {
            (200, _) => {}
            // `resyncRequired` and its kin: the server dropped what the link stands for.
            (410, true) => return Err(ReplicaError::AnchorExpired),
            _ => return Err(read_error(&response)),
        }
        let page: DeltaPage = serde_json::from_slice(&response.body).map_err(|_| busy())?;
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
        self.addr
            .path(None, Some("delta"), "")
            .ok_or(ReplicaError::Gone)
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
            let url = self.addr.under_app(&names[..at], Some("children"), "");
            let body = format!(
                r#"{{"name":{},"folder":{{}},"@microsoft.graph.conflictBehavior":"fail"}}"#,
                json_string(name)
            );
            let request = HttpRequest::new(Method::Post, url.ok_or(ReplicaError::Gone)?)
                .with_header("Content-Type", "application/json")
                .with_body(body);
            let response = self.send(request).await?;
            // 409: the level is already there.
            if !matches!(response.status.0, 200 | 201 | 409) {
                return Err(read_error(&response));
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
                state.folders.insert(
                    item.id.clone(),
                    (
                        item.name.clone().unwrap_or_default(),
                        item.parent_id().map(str::to_owned),
                    ),
                );
            }
        }
        let mut changes = Vec::new();
        for item in items {
            match item.kind() {
                Kind::Deleted => changes.extend(self.gone(&item.id)),
                Kind::File => changes.extend(self.upsert(&item).await?),
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
    /// folder.
    fn gone(&self, id: &str) -> Vec<Change> {
        let mut state = self.state();
        let was_folder = state.folders.remove(id).is_some();
        if !was_folder {
            state.files.remove(id);
            return vec![self.tombstone(id)];
        }
        let held: Vec<String> = state
            .files
            .iter()
            .filter(|(_, parent)| below(&state.folders, parent, id))
            .map(|(file, _)| file.clone())
            .collect();
        // Folders inside it are gone with it.
        let inner: Vec<String> = state
            .folders
            .keys()
            .filter(|folder| below(&state.folders, folder, id))
            .cloned()
            .collect();
        for folder in inner {
            state.folders.remove(&folder);
        }
        for file in &held {
            state.files.remove(file);
        }
        held.iter().map(|file| self.tombstone(file)).collect()
    }

    async fn upsert(&self, item: &DriveItem) -> Result<Option<Change>, ReplicaError> {
        let (Some(parent), Some(name)) = (item.parent_id(), item.name.as_deref()) else {
            return Ok(None);
        };
        let Some(path) = self.path_of(parent, name).await? else {
            // Outside the dataset's folder.
            return Ok(None);
        };
        self.state()
            .files
            .insert(item.id.clone(), parent.to_owned());
        Ok(Some(Change::Upsert(RemoteItem {
            id: RemoteId(item.id.clone()),
            version: RemoteVersion(item.etag.clone().unwrap_or_default()),
            path,
            size: Bytes(item.size.unwrap_or(0)),
            hash: item.hash(),
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
            let known = self.state().folders.get(&at).cloned();
            let (folder, up) = match known {
                Some(known) => known,
                None => match self.item_now(&at).await? {
                    Some(item) => {
                        let known = (
                            item.name.clone().unwrap_or_default(),
                            item.parent_id().map(str::to_owned),
                        );
                        self.state().folders.insert(at.clone(), known.clone());
                        known
                    }
                    None => return Ok(None),
                },
            };
            let Some(up) = up else { return Ok(None) };
            names.push(folder);
            at = up;
        }
        Ok(None)
    }
}

/// Whether the folder `at` is `ancestor` or inside it, by the folders seen.
fn below(
    folders: &std::collections::HashMap<String, (String, Option<String>)>,
    at: &str,
    ancestor: &str,
) -> bool {
    let mut current = Some(at);
    for _ in 0..DEPTH {
        match current {
            Some(id) if id == ancestor => return true,
            Some(id) => current = folders.get(id).and_then(|(_, up)| up.as_deref()),
            None => return false,
        }
    }
    false
}

/// A JSON string literal.
pub(crate) fn json_string(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}
