//! What Drive's JSON says, read as far as the replica needs.

use porter_sync::ContentHash;
use serde::Deserialize;

/// The mime type of a folder.
pub const FOLDER: &str = "application/vnd.google-apps.folder";

/// A file resource.
#[derive(Debug, Clone, Deserialize)]
pub struct File {
    pub id: String,
    pub name: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime: Option<String>,
    pub parents: Option<Vec<String>>,
    pub version: Option<String>,
    #[serde(rename = "md5Checksum")]
    pub md5: Option<String>,
    /// Drive sends sizes as strings.
    pub size: Option<String>,
    pub trashed: Option<bool>,
}

impl File {
    /// Whether it is a folder.
    pub fn is_folder(&self) -> bool {
        self.mime.as_deref() == Some(FOLDER)
    }

    /// Its first parent's id.
    pub fn parent_id(&self) -> Option<&str> {
        self.parents.as_ref()?.first().map(String::as_str)
    }

    /// Its size in bytes (zero when Drive sends none, as it does for a folder).
    pub fn bytes(&self) -> u64 {
        self.size
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    /// Its MD5 as hex.
    pub fn hash(&self) -> Option<ContentHash> {
        self.md5.clone().map(ContentHash)
    }

    /// Whether it is in the trash.
    pub fn in_trash(&self) -> bool {
        self.trashed == Some(true)
    }
}

/// One page of `files.list`.
#[derive(Debug, Deserialize)]
pub struct FileList {
    #[serde(default)]
    pub files: Vec<File>,
    #[serde(rename = "nextPageToken")]
    pub next: Option<String>,
}

/// One entry of `changes.list`.
#[derive(Debug, Deserialize)]
pub struct Change {
    #[serde(rename = "fileId")]
    pub file_id: Option<String>,
    pub removed: Option<bool>,
    pub file: Option<File>,
}

/// One page of `changes.list`.
#[derive(Debug, Deserialize)]
pub struct ChangeList {
    #[serde(default)]
    pub changes: Vec<Change>,
    #[serde(rename = "nextPageToken")]
    pub next: Option<String>,
    #[serde(rename = "newStartPageToken")]
    pub new_start: Option<String>,
}

/// `changes.getStartPageToken`.
#[derive(Debug, Deserialize)]
pub struct StartToken {
    #[serde(rename = "startPageToken")]
    pub token: String,
}

/// `about`, as far as the quota.
#[derive(Debug, Deserialize)]
pub struct About {
    #[serde(rename = "storageQuota")]
    pub quota: Option<Quota>,
}

/// The account's storage quota; Drive sends the numbers as strings and no `limit` for unlimited.
#[derive(Debug, Deserialize)]
pub struct Quota {
    pub limit: Option<String>,
    pub usage: Option<String>,
}

/// An error body, as far as the reasons.
#[derive(Debug, Deserialize)]
pub struct ErrorBody {
    pub error: Option<ErrorInner>,
}

/// The inside of an error body.
#[derive(Debug, Deserialize)]
pub struct ErrorInner {
    #[serde(default)]
    pub errors: Vec<Reason>,
}

/// One reason.
#[derive(Debug, Deserialize)]
pub struct Reason {
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_read_as_far_as_the_replica_needs() {
        let file: File = serde_json::from_str(
            r#"{"id":"1","name":"a.txt","mimeType":"text/plain","parents":["p"],"version":"7",
                "md5Checksum":"d41d8cd98f00b204e9800998ecf8427e","size":"12","trashed":false}"#,
        )
        .expect("file");
        assert_eq!(
            (
                file.is_folder(),
                file.parent_id(),
                file.bytes(),
                file.in_trash()
            ),
            (false, Some("p"), 12, false)
        );
        assert_eq!(
            file.hash(),
            Some(ContentHash("d41d8cd98f00b204e9800998ecf8427e".into()))
        );
        let folder: File = serde_json::from_str(&format!(r#"{{"id":"2","mimeType":"{FOLDER}"}}"#))
            .expect("folder");
        assert_eq!(
            (folder.is_folder(), folder.bytes(), folder.hash()),
            (true, 0, None)
        );
    }
}
