//! The change feed. The first listing is `files.list` of the root's space (the app data folder,
//! or My Drive: `spaces=drive`, the user's own corpus), page by page, taken after
//! `changes.getStartPageToken` so nothing between the two is missed; after it, `changes.list`
//! from the start token (in My Drive restricted to it). The anchor says where in that the replica
//! is: `changes:<page token>` or, during the first listing, `list:<start token>:<files page
//! token>`.
//!
//! A change names a file by id and parent id and carries no path, so a path is built by walking
//! the folders seen up to the dataset's folder, asking the server for one that has not been
//! seen; a file whose parents do not lead there (shared with the person, not added to My Drive)
//! is left out. A deleted folder is reported alone: the files known to have been in it are given
//! tombstones here.
//!
//! An app folder's feed is its files. My Drive's is its folders too, and Google's own documents
//! and shortcuts as items that open online.

use crate::addr::{Root, item_path, literal};
use crate::json::{Change as Wire, ChangeList, File, FileList, StartToken};
use crate::refuse::read_error;
use crate::replica::GdriveReplica;
use porter_http::http::header::CONTENT_TYPE;
use porter_http::http::{Method, Response};
use porter_http::{Http, request};
use porter_ids::{Bytes, UnixSeconds, WebUrl};
use porter_sync::DELETED;
use porter_sync::status::unreached;
use porter_sync::{
    Anchor, Change, ChangePage, Cursor, Folder, ItemKind, ItemPath, More, RemoteId, RemoteItem,
    RemoteVersion, ReplicaError, RetryAfter, Tombstone,
};

/// How many folders a path may have above a file before the walk gives up.
const DEPTH: usize = 64;

fn busy() -> ReplicaError {
    ReplicaError::Transient(RetryAfter(30))
}

/// Where in the feed an anchor says the replica is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Place {
    /// `changes.list` from this token.
    Changes(String),
    /// The first listing: the token to start `changes.list` from after it, and the page to read.
    Listing { start: String, page: String },
}

impl Place {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        if let Some(token) = text.strip_prefix("changes:") {
            return (!token.is_empty()).then(|| Place::Changes(token.to_owned()));
        }
        let (start, page) = text.strip_prefix("list:")?.split_once(':')?;
        (!start.is_empty() && !page.is_empty()).then(|| Place::Listing {
            start: start.to_owned(),
            page: page.to_owned(),
        })
    }

    pub(crate) fn text(&self) -> String {
        match self {
            Place::Changes(token) => format!("changes:{token}"),
            Place::Listing { start, page } => format!("list:{start}:{page}"),
        }
    }
}

/// The page sizes Drive takes (1000 at most).
fn size(page: usize) -> String {
    page.clamp(1, 1000).to_string()
}

impl<H: Http> GdriveReplica<H> {
    pub(crate) async fn feed(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
        match from {
            Cursor::Start => {
                // A new listing starts over: what was seen may be gone.
                *self.state() = crate::replica::State::default();
                self.root_id(true).await?;
                let start = self.start_token().await?;
                self.list_page(&start, None).await
            }
            Cursor::At(Anchor(text)) => match Place::parse(&text) {
                Some(Place::Changes(token)) => self.change_page(&token).await,
                Some(Place::Listing { start, page }) => self.list_page(&start, Some(&page)).await,
                None => Err(ReplicaError::AnchorExpired),
            },
        }
    }

    async fn start_token(&self) -> Result<String, ReplicaError> {
        let response = self
            .get_url(self.addr.api("changes/startPageToken", &[]))
            .await?;
        if response.status().as_u16() != 200 {
            return Err(read_error(&response, self.clock.now()));
        }
        let token: StartToken = serde_json::from_slice(response.body()).map_err(|_| busy())?;
        Ok(token.token)
    }

    /// An answer to a request that resumed an anchor: a token Drive refuses is an expired anchor.
    fn resumed(response: &Response<Vec<u8>>, now: UnixSeconds) -> ReplicaError {
        match response.status().as_u16() {
            400 | 404 | 410 => ReplicaError::AnchorExpired,
            _ => read_error(response, now),
        }
    }

    async fn list_page(&self, start: &str, page: Option<&str>) -> Result<ChangePage, ReplicaError> {
        let root = self.addr.root();
        let fields = format!("nextPageToken,files({})", root.fields());
        let page_size = size(self.page);
        let mut pairs = vec![
            ("spaces", root.space()),
            ("q", "trashed = false"),
            ("pageSize", page_size.as_str()),
            ("fields", fields.as_str()),
        ];
        if root == Root::Drive {
            // The person's own files, not those of shared drives.
            pairs.push(("corpora", "user"));
        }
        if let Some(token) = page {
            pairs.push(("pageToken", token));
        }
        let response = self.get_url(self.addr.api("files", &pairs)).await?;
        match (response.status().as_u16(), page.is_some()) {
            (200, _) => {}
            (_, true) => return Err(Self::resumed(&response, self.clock.now())),
            _ => return Err(read_error(&response, self.clock.now())),
        }
        let list: FileList = serde_json::from_slice(response.body()).map_err(|_| busy())?;
        let changes = self.apply_files(list.files).await?;
        let (place, more) = match list.next {
            Some(next) => (
                Place::Listing {
                    start: start.to_owned(),
                    page: next,
                },
                More::More,
            ),
            None => (Place::Changes(start.to_owned()), More::Done),
        };
        Ok(ChangePage {
            changes,
            next: Anchor(place.text()),
            more,
        })
    }

    async fn change_page(&self, token: &str) -> Result<ChangePage, ReplicaError> {
        let root = self.addr.root();
        let fields = format!(
            "nextPageToken,newStartPageToken,changes(fileId,removed,file({}))",
            root.fields()
        );
        let page_size = size(self.page);
        let mut pairs = vec![
            ("pageToken", token),
            ("spaces", root.space()),
            ("includeRemoved", "true"),
            ("pageSize", page_size.as_str()),
            ("fields", fields.as_str()),
        ];
        if root == Root::Drive {
            pairs.push(("restrictToMyDrive", "true"));
        }
        let response = self.get_url(self.addr.api("changes", &pairs)).await?;
        if response.status().as_u16() != 200 {
            return Err(Self::resumed(&response, self.clock.now()));
        }
        let list: ChangeList = serde_json::from_slice(response.body()).map_err(|_| busy())?;
        let (next, more) = match (list.next, list.new_start) {
            (Some(next), _) => (next, More::More),
            (None, Some(start)) => (start, More::Done),
            (None, None) => return Err(busy()),
        };
        let changes = self.apply_changes(list.changes).await?;
        Ok(ChangePage {
            changes,
            next: Anchor(Place::Changes(next).text()),
            more,
        })
    }

    /// The root's id (the app data folder's, or My Drive's).
    async fn top_id(&self) -> Result<String, ReplicaError> {
        let url = self
            .addr
            .file(self.addr.root().alias(), &[("fields", "id")]);
        let response = self.get_url(url).await?;
        if response.status().as_u16() != 200 {
            return Err(read_error(&response, self.clock.now()));
        }
        let file: File = serde_json::from_slice(response.body()).map_err(|_| busy())?;
        Ok(file.id)
    }

    /// The live files and folders in `parent` named `name`.
    pub(crate) async fn named_in(
        &self,
        parent: &str,
        name: &str,
    ) -> Result<Vec<File>, ReplicaError> {
        let q = format!(
            "{} in parents and name = {} and trashed = false",
            literal(parent),
            literal(name)
        );
        let fields = format!("files({})", crate::addr::FILE_FIELDS);
        let root = self.addr.root();
        let mut pairs = vec![
            ("spaces", root.space()),
            ("q", q.as_str()),
            ("fields", fields.as_str()),
        ];
        if root == Root::Drive {
            pairs.push(("corpora", "user"));
        }
        let response = self.get_url(self.addr.api("files", &pairs)).await?;
        if response.status().as_u16() != 200 {
            return Err(read_error(&response, self.clock.now()));
        }
        let list: FileList = serde_json::from_slice(response.body()).map_err(|_| busy())?;
        Ok(list.files)
    }

    /// Makes the folder `name` in `parent`.
    pub(crate) async fn make_folder(
        &self,
        parent: &str,
        name: &str,
    ) -> Result<String, ReplicaError> {
        let body = serde_json::json!({
            "name": name,
            "mimeType": crate::json::FOLDER,
            "parents": [parent],
        })
        .to_string();
        let url = self.addr.api("files", &[("fields", "id")]);
        let make = request(Method::POST, &url.ok_or(ReplicaError::Gone)?)
            .header(CONTENT_TYPE, "application/json")
            .body(body.into_bytes())
            .map_err(|e| unreached(e.into()))?;
        let response = self.send(make).await?;
        if response.status().as_u16() != 200 {
            return Err(read_error(&response, self.clock.now()));
        }
        let file: File = serde_json::from_slice(response.body()).map_err(|_| busy())?;
        Ok(file.id)
    }

    /// The folder `names` below `from`, made level by level when `make` is set.
    pub(crate) async fn folder_below(
        &self,
        from: &str,
        names: &[String],
        make: bool,
    ) -> Result<String, ReplicaError> {
        let mut at = from.to_owned();
        for name in names {
            let found = self
                .named_in(&at, name)
                .await?
                .into_iter()
                .find(File::is_folder)
                .map(|f| f.id);
            at = match (found, make) {
                (Some(id), _) => id,
                (None, true) => self.make_folder(&at, name).await?,
                (None, false) => return Err(ReplicaError::Gone),
            };
        }
        Ok(at)
    }

    /// The dataset folder's id; with `make`, the folder is created if it is missing.
    pub(crate) async fn root_id(&self, make: bool) -> Result<String, ReplicaError> {
        if let Some(id) = self.state().root.clone() {
            return Ok(id);
        }
        let top = self.top_id().await?;
        let id = self.folder_below(&top, self.addr.folder(), make).await?;
        self.state().root = Some(id.clone());
        Ok(id)
    }

    /// Reads a listing page's files into changes.
    async fn apply_files(&self, files: Vec<File>) -> Result<Vec<Change>, ReplicaError> {
        self.remember_folders(files.iter());
        let mut changes = Vec::new();
        for file in &files {
            changes.extend(self.live(file).await?);
        }
        Ok(changes)
    }

    /// The change a live file is, if the dataset carries it: below an app folder its files,
    /// in My Drive its folders too and what opens online.
    async fn live(&self, file: &File) -> Result<Vec<Change>, ReplicaError> {
        let kind = match self.addr.root() {
            Root::AppFolder if file.is_folder() => return Ok(Vec::new()),
            Root::AppFolder => ItemKind::File,
            Root::Drive => match file.kind() {
                Some(kind) => kind,
                None => return Ok(Vec::new()),
            },
        };
        self.upsert(file, kind).await
    }

    fn remember_folders<'a>(&self, files: impl Iterator<Item = &'a File>) {
        let mut state = self.state();
        for folder in files.filter(|f| f.is_folder() && !f.in_trash()) {
            state
                .folders
                .insert_folder(RemoteId(folder.id.clone()), folder_of(folder));
        }
    }

    /// Reads a change page's entries into changes, oldest first.
    async fn apply_changes(&self, entries: Vec<Wire>) -> Result<Vec<Change>, ReplicaError> {
        // Folders first: a file's parent may come after it in the page.
        self.remember_folders(entries.iter().filter_map(|c| c.file.as_ref()));
        let mut changes = Vec::new();
        for entry in entries {
            let Some(id) = entry
                .file_id
                .clone()
                .or_else(|| entry.file.as_ref().map(|f| f.id.clone()))
            else {
                continue;
            };
            match entry.file {
                Some(file) if entry.removed != Some(true) && !file.in_trash() => {
                    changes.extend(self.live(&file).await?);
                }
                _ => changes.extend(self.gone(&id)),
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
    /// folder (in My Drive, which reports folders as items, of the folders inside it and its own
    /// too, inner ones first).
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

    async fn upsert(&self, file: &File, kind: ItemKind) -> Result<Vec<Change>, ReplicaError> {
        let (Some(parent), Some(name)) = (file.parent_id(), file.name.as_deref()) else {
            return Ok(Vec::new());
        };
        let id = RemoteId(file.id.clone());
        let Some(path) = self.path_of(parent, name).await? else {
            // Outside the dataset's folder: an item moved out of it is gone from the dataset
            // (in My Drive a folder takes what it held with it).
            if kind == ItemKind::Folder {
                return Ok(self.gone_folder(&id));
            }
            let known = self.state().folders.remove_file(&id);
            return Ok(known
                .then(|| self.tombstone(&file.id))
                .into_iter()
                .collect());
        };
        if kind != ItemKind::Folder {
            self.state()
                .folders
                .insert_file(id.clone(), RemoteId(parent.to_owned()));
        }
        Ok(vec![Change::Upsert(RemoteItem {
            id,
            version: RemoteVersion(file.version.clone().unwrap_or_default()),
            path,
            size: Bytes(file.bytes()),
            hash: file.hash(),
            kind,
            modified: file.modified.as_deref().and_then(UnixSeconds::from_rfc3339),
        })])
    }

    /// The tombstones of a folder that left the dataset, if it was in it.
    fn gone_folder(&self, id: &RemoteId) -> Vec<Change> {
        // The page's folders were remembered before its items, so a folder that moved out is
        // still in the tree, now under its new parent; one never seen is nothing to remove.
        let known = self.state().folders.folder(id).is_some();
        match known {
            true => self.gone(&id.0),
            false => Vec::new(),
        }
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
                None => match self.file_now(&at).await? {
                    Some(file) if file.is_folder() && !file.in_trash() => {
                        let known = folder_of(&file);
                        self.state().folders.insert_folder(id, known.clone());
                        known
                    }
                    _ => return Ok(None),
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

/// A folder as the tree keeps it.
fn folder_of(file: &File) -> Folder {
    Folder {
        name: file.name.clone().unwrap_or_default(),
        parent: file.parent_id().map(|p| RemoteId(p.to_owned())),
    }
}

/// A web URL of a Location or other link the server handed back, if it parses.
pub(crate) fn link(text: &str) -> Option<WebUrl> {
    WebUrl::parse(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_anchor_names_a_place_in_the_feed_and_back() {
        const CASES: &[(&str, Option<Place>)] = &[
            ("changes:42", Some(Place::Changes(String::new()))),
            ("changes:", None),
            (
                "list:7:abc",
                Some(Place::Listing {
                    start: String::new(),
                    page: String::new(),
                }),
            ),
            ("list:7:", None),
            ("list::x", None),
            ("delta:1", None),
            ("", None),
        ];
        for (text, want) in CASES {
            let got = Place::parse(text);
            assert_eq!(got.is_some(), want.is_some(), "{text:?}");
            if let Some(place) = got {
                assert_eq!(place.text(), *text);
            }
        }
        // Tokens keep their own colons.
        assert_eq!(
            Place::parse("list:7:a:b"),
            Some(Place::Listing {
                start: "7".into(),
                page: "a:b".into()
            })
        );
    }
}
