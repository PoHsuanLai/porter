//! The change feed. The first listing is `files.list` of the app data folder space, page by
//! page, taken after `changes.getStartPageToken` so nothing between the two is missed; after
//! it, `changes.list` from the start token. The anchor says where in that the replica is:
//! `changes:<page token>` or, during the first listing, `list:<start token>:<files page token>`.
//!
//! A change names a file by id and parent id and carries no path, so a path is built by walking
//! the folders seen up to the dataset's folder, asking the server for one that has not been
//! seen. A deleted folder is reported alone: the files known to have been in it are given
//! tombstones here.

use crate::addr::{FILE_FIELDS, item_path, literal};
use crate::json::{Change as Wire, ChangeList, File, FileList, StartToken};
use crate::refuse::read_error;
use crate::replica::GdriveReplica;
use porter_core::{Bytes, WebUrl};
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use porter_sync::{
    Anchor, Change, ChangePage, Cursor, ItemPath, More, RemoteId, RemoteItem, RemoteVersion,
    ReplicaError, RetryAfter, Tombstone,
};
use std::collections::HashMap;
use storage_webdav::DELETED;

/// How many folders a path may have above a file before the walk gives up.
const DEPTH: usize = 64;

/// The space the replica lives in.
const SPACE: &str = "appDataFolder";

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
        if response.status.0 != 200 {
            return Err(read_error(&response));
        }
        let token: StartToken = serde_json::from_slice(&response.body).map_err(|_| busy())?;
        Ok(token.token)
    }

    /// An answer to a request that resumed an anchor: a token Drive refuses is an expired anchor.
    fn resumed(response: &HttpResponse) -> ReplicaError {
        match response.status.0 {
            400 | 404 | 410 => ReplicaError::AnchorExpired,
            _ => read_error(response),
        }
    }

    async fn list_page(&self, start: &str, page: Option<&str>) -> Result<ChangePage, ReplicaError> {
        let fields = format!("nextPageToken,files({FILE_FIELDS})");
        let page_size = size(self.page);
        let mut pairs = vec![
            ("spaces", SPACE),
            ("q", "trashed = false"),
            ("pageSize", page_size.as_str()),
            ("fields", fields.as_str()),
        ];
        if let Some(token) = page {
            pairs.push(("pageToken", token));
        }
        let response = self.get_url(self.addr.api("files", &pairs)).await?;
        match (response.status.0, page.is_some()) {
            (200, _) => {}
            (_, true) => return Err(Self::resumed(&response)),
            _ => return Err(read_error(&response)),
        }
        let list: FileList = serde_json::from_slice(&response.body).map_err(|_| busy())?;
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
        let fields =
            format!("nextPageToken,newStartPageToken,changes(fileId,removed,file({FILE_FIELDS}))");
        let page_size = size(self.page);
        let pairs = [
            ("pageToken", token),
            ("spaces", SPACE),
            ("includeRemoved", "true"),
            ("pageSize", page_size.as_str()),
            ("fields", fields.as_str()),
        ];
        let response = self.get_url(self.addr.api("changes", &pairs)).await?;
        if response.status.0 != 200 {
            return Err(Self::resumed(&response));
        }
        let list: ChangeList = serde_json::from_slice(&response.body).map_err(|_| busy())?;
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

    /// The app data folder's id.
    async fn app_root(&self) -> Result<String, ReplicaError> {
        let url = self.addr.file(crate::addr::ROOT_ALIAS, &[("fields", "id")]);
        let response = self.get_url(url).await?;
        if response.status.0 != 200 {
            return Err(read_error(&response));
        }
        let file: File = serde_json::from_slice(&response.body).map_err(|_| busy())?;
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
        let fields = format!("files({FILE_FIELDS})");
        let pairs = [
            ("spaces", SPACE),
            ("q", q.as_str()),
            ("fields", fields.as_str()),
        ];
        let response = self.get_url(self.addr.api("files", &pairs)).await?;
        if response.status.0 != 200 {
            return Err(read_error(&response));
        }
        let list: FileList = serde_json::from_slice(&response.body).map_err(|_| busy())?;
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
        let request = HttpRequest::new(Method::Post, url.ok_or(ReplicaError::Gone)?)
            .with_header("Content-Type", "application/json")
            .with_body(body);
        let response = self.send(request).await?;
        if response.status.0 != 200 {
            return Err(read_error(&response));
        }
        let file: File = serde_json::from_slice(&response.body).map_err(|_| busy())?;
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
        let app = self.app_root().await?;
        let id = self.folder_below(&app, self.addr.folder(), make).await?;
        self.state().root = Some(id.clone());
        Ok(id)
    }

    /// Reads a listing page's files into changes.
    async fn apply_files(&self, files: Vec<File>) -> Result<Vec<Change>, ReplicaError> {
        self.remember_folders(files.iter());
        let mut changes = Vec::new();
        for file in files.iter().filter(|f| !f.is_folder()) {
            changes.extend(self.upsert(file).await?);
        }
        Ok(changes)
    }

    fn remember_folders<'a>(&self, files: impl Iterator<Item = &'a File>) {
        let mut state = self.state();
        for folder in files.filter(|f| f.is_folder() && !f.in_trash()) {
            state.folders.insert(
                folder.id.clone(),
                (
                    folder.name.clone().unwrap_or_default(),
                    folder.parent_id().map(str::to_owned),
                ),
            );
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
                    if !file.is_folder() {
                        changes.extend(self.upsert(&file).await?);
                    }
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

    async fn upsert(&self, file: &File) -> Result<Option<Change>, ReplicaError> {
        let (Some(parent), Some(name)) = (file.parent_id(), file.name.as_deref()) else {
            return Ok(None);
        };
        let Some(path) = self.path_of(parent, name).await? else {
            // Outside the dataset's folder: a file moved out of it is gone from the dataset.
            let known = self.state().files.remove(&file.id).is_some();
            return Ok(known.then(|| self.tombstone(&file.id)));
        };
        self.state()
            .files
            .insert(file.id.clone(), parent.to_owned());
        Ok(Some(Change::Upsert(RemoteItem {
            id: RemoteId(file.id.clone()),
            version: RemoteVersion(file.version.clone().unwrap_or_default()),
            path,
            size: Bytes(file.bytes()),
            hash: file.hash(),
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
                None => match self.file_now(&at).await? {
                    Some(file) if file.is_folder() && !file.in_trash() => {
                        let known = (
                            file.name.clone().unwrap_or_default(),
                            file.parent_id().map(str::to_owned),
                        );
                        self.state().folders.insert(at.clone(), known.clone());
                        known
                    }
                    _ => return Ok(None),
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
fn below(folders: &HashMap<String, (String, Option<String>)>, at: &str, ancestor: &str) -> bool {
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
