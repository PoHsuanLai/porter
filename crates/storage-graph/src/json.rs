//! What Graph's JSON says, read as far as the replica needs.

use porter_sync::ContentHash;
use serde::Deserialize;
use serde::de::IgnoredAny;

/// A drive item, from a listing, a delta page or a write's answer.
#[derive(Debug, Deserialize)]
pub struct DriveItem {
    pub id: String,
    pub name: Option<String>,
    pub size: Option<u64>,
    #[serde(rename = "eTag")]
    pub etag: Option<String>,
    #[serde(rename = "parentReference")]
    pub parent: Option<Parent>,
    pub file: Option<FileFacet>,
    pub folder: Option<IgnoredAny>,
    pub deleted: Option<IgnoredAny>,
    #[serde(rename = "remoteItem")]
    pub remote: Option<IgnoredAny>,
}

/// Where an item is.
#[derive(Debug, Deserialize)]
pub struct Parent {
    pub id: Option<String>,
}

/// What a file adds.
#[derive(Debug, Deserialize)]
pub struct FileFacet {
    pub hashes: Option<Hashes>,
}

/// The hashes OneDrive reports.
#[derive(Debug, Deserialize)]
pub struct Hashes {
    #[serde(rename = "quickXorHash")]
    pub quick_xor: Option<String>,
}

/// What a delta item is, for the replica.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Gone.
    Deleted,
    /// A folder.
    Folder,
    /// A file of this drive.
    File,
    /// A shortcut, a package, or something else the replica does not sync.
    Other,
}

impl DriveItem {
    /// What it is.
    pub fn kind(&self) -> Kind {
        match (&self.deleted, &self.folder, &self.file, &self.remote) {
            (Some(_), ..) => Kind::Deleted,
            (None, _, _, Some(_)) => Kind::Other,
            (None, Some(_), ..) => Kind::Folder,
            (None, None, Some(_), None) => Kind::File,
            _ => Kind::Other,
        }
    }

    /// Its parent's id.
    pub fn parent_id(&self) -> Option<&str> {
        self.parent.as_ref()?.id.as_deref()
    }

    /// Its QuickXorHash as hex (Graph sends it as base64 of 20 bytes).
    pub fn hash(&self) -> Option<ContentHash> {
        use base64::Engine;
        let text = self.file.as_ref()?.hashes.as_ref()?.quick_xor.as_ref()?;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(text)
            .ok()?;
        Some(ContentHash(
            raw.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        ))
    }
}

/// One page of a delta query.
#[derive(Debug, Deserialize)]
pub struct DeltaPage {
    pub value: Vec<DriveItem>,
    #[serde(rename = "@odata.nextLink")]
    pub next: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    pub delta: Option<String>,
}

/// An upload session.
#[derive(Debug, Deserialize)]
pub struct Session {
    #[serde(rename = "uploadUrl")]
    pub upload_url: String,
}

/// `GET /me/drive`.
#[derive(Debug, Deserialize)]
pub struct DriveInfo {
    pub quota: Option<QuotaInfo>,
}

/// The drive's quota.
#[derive(Debug, Deserialize)]
pub struct QuotaInfo {
    pub total: Option<u64>,
    pub used: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: &str) -> DriveItem {
        serde_json::from_str(json).expect("item")
    }

    #[test]
    fn an_item_is_classed_by_its_facets() {
        const CASES: &[(&str, &str, Kind)] = &[
            ("file", r#"{"id":"1","file":{}}"#, Kind::File),
            (
                "folder",
                r#"{"id":"2","folder":{"childCount":1}}"#,
                Kind::Folder,
            ),
            (
                "deleted",
                r#"{"id":"3","deleted":{"state":"deleted"}}"#,
                Kind::Deleted,
            ),
            (
                "deleted file",
                r#"{"id":"4","file":{},"deleted":{}}"#,
                Kind::Deleted,
            ),
            (
                "shortcut",
                r#"{"id":"5","file":{},"remoteItem":{"id":"x"}}"#,
                Kind::Other,
            ),
            ("bare", r#"{"id":"6"}"#, Kind::Other),
        ];
        for (name, json, kind) in CASES {
            assert_eq!(item(json).kind(), *kind, "{name}");
        }
    }

    #[test]
    fn the_quick_xor_hash_is_read_as_hex() {
        let with = item(r#"{"id":"1","file":{"hashes":{"quickXorHash":"AQID"}}}"#);
        assert_eq!(with.hash(), Some(ContentHash("010203".into())));
        assert_eq!(item(r#"{"id":"1","file":{}}"#).hash(), None);
        let bad = item(r#"{"id":"1","file":{"hashes":{"quickXorHash":"!!"}}}"#);
        assert_eq!(bad.hash(), None);
    }
}
