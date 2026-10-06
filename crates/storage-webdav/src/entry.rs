//! What a multistatus says about each resource: its id, whether it is a folder, its etag and
//! its size.

use crate::path::Root;
use porter_core::Bytes;
use porter_dav::{Multistatus, names};
use porter_sync::{RemoteId, RemoteItem, RemoteVersion};

/// `DAV:getcontentlength`.
pub const CONTENT_LENGTH: &str = "DAV:getcontentlength";

/// What a resource is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A file.
    File,
    /// A collection.
    Folder,
}

/// One resource of a multistatus, inside the dataset's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its id.
    pub id: RemoteId,
    /// File or folder.
    pub kind: Kind,
    /// Its etag, as the server wrote it.
    pub version: RemoteVersion,
    /// Its size; zero for a folder or when the server did not say.
    pub size: Bytes,
}

impl Entry {
    /// The change feed's item for a file; `None` for a folder or the dataset's folder.
    pub fn item(&self, root: &Root) -> Option<RemoteItem> {
        (self.kind == Kind::File).then_some(())?;
        Some(RemoteItem {
            id: self.id.clone(),
            version: self.version.clone(),
            path: root.item_path(&self.id)?,
            size: self.size,
            hash: None,
        })
    }
}

/// The resources a multistatus reports as present (no 404 and no failed status), inside
/// the folder, in the server's order.
pub fn entries(root: &Root, status: &Multistatus) -> Vec<Entry> {
    status
        .responses
        .iter()
        .filter(|r| r.status.is_none_or(|code| (200..300).contains(&code)))
        .filter_map(|r| {
            let id = root.id_of(&r.href)?;
            let folder = r.href.ends_with('/')
                || r.found(names::RESOURCETYPE)
                    .is_some_and(|t| t.split_whitespace().any(|k| k == "DAV:collection"));
            let size = r
                .found(CONTENT_LENGTH)
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(0);
            // A server that sends no etag still has a version: the one thing it did send.
            let version = r
                .found(names::GETETAG)
                .filter(|v| !v.is_empty())
                .map_or_else(|| format!("len-{size}"), str::to_owned);
            Some(Entry {
                id,
                kind: if folder { Kind::Folder } else { Kind::File },
                version: RemoteVersion(version),
                size: Bytes(size),
            })
        })
        .collect()
}
