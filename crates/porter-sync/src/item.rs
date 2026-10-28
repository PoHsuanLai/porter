//! A remote item and the versions a write is checked against.

use porter_ids::{Bytes, UnixSeconds, WebUrl};
use serde::{Deserialize, Serialize};

/// A replica's own id for an item (a Drive file id, a WebDAV href).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RemoteId(pub String);

/// A replica's version of an item (an etag, a revision id).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RemoteVersion(pub String);

/// A path inside the dataset's folder (`Originals/2026/09/IMG_1234.HEIC`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemPath(pub String);

/// A content hash in the form the replica reports it (design/31 `HashKind`), hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(pub String);

/// The version a write was based on. A mismatch is a conflict, never an overwrite.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum BaseVersion {
    /// The writer believes the item does not exist yet.
    Absent,
    /// The writer last saw this version.
    At(RemoteVersion),
}

/// What an item is. A replica of an app folder reports files only; one of a whole drive reports
/// its folders too, and the documents that live only online.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ItemKind {
    /// A file whose bytes `fetch` reads.
    #[default]
    File,
    /// A folder; it has no bytes.
    Folder,
    /// A document that lives only online (a Google Doc, a shortcut): it has no bytes to fetch and
    /// is opened at `url`.
    Online {
        /// Where it opens in a browser.
        url: WebUrl,
        /// The MIME type a copy of it is saved as (an Office type, or PNG for a drawing), or
        /// `None` when it cannot be saved as a file (a form, a site, a map).
        export: Option<String>,
    },
}

impl ItemKind {
    /// Whether it is a file with bytes.
    pub fn is_file(&self) -> bool {
        matches!(self, ItemKind::File)
    }
}

/// One live item as the change feed reports it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemoteItem {
    /// Its id.
    pub id: RemoteId,
    /// Its current version.
    pub version: RemoteVersion,
    /// Where it is.
    pub path: ItemPath,
    /// Its size.
    pub size: Bytes,
    /// Its content hash, when the replica reports one.
    pub hash: Option<ContentHash>,
    /// What it is; a file when the form names none.
    #[serde(default)]
    pub kind: ItemKind,
    /// When it last changed, as the replica says; none when it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<UnixSeconds>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_pins_its_form_and_an_item_that_names_none_is_a_file() {
        let online = ItemKind::Online {
            url: WebUrl::parse("https://docs.example.test/d/1").expect("url"),
            export: Some("image/png".into()),
        };
        let json = serde_json::to_string(&online).expect("json");
        assert_eq!(
            json,
            r#"{"kind":"online","v":{"url":"https://docs.example.test/d/1","export":"image/png"}}"#
        );
        assert_eq!(
            serde_json::from_str::<ItemKind>(&json).expect("kind"),
            online
        );
        let old = r#"{"id":"1","version":"v","path":"a.txt","size":3,"hash":null}"#;
        let item: RemoteItem = serde_json::from_str(old).expect("item");
        assert_eq!(item.kind, ItemKind::File);
        assert!(item.kind.is_file() && !ItemKind::Folder.is_file() && !online.is_file());
    }
}
