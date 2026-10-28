//! What Drive's JSON says, read as far as the replica needs.

use porter_ids::WebUrl;
use porter_sync::{ContentHash, ItemKind};
use serde::Deserialize;

/// The mime type of a folder.
pub const FOLDER: &str = "application/vnd.google-apps.folder";

/// The mime type of a shortcut to another file.
pub const SHORTCUT: &str = "application/vnd.google-apps.shortcut";

/// What Google's own documents' mime types start with: they have no bytes, only a page.
const NATIVE: &str = "application/vnd.google-apps.";

/// Where a file opens when Drive names no page for it.
const OPEN: &str = "https://drive.google.com/open?id=";

/// The type each kind of Google document is saved as when a copy is made: the Office type, or
/// PNG for a drawing. Forms, sites, maps and the rest have none.
const EXPORTS: &[(&str, &str)] = &[
    (
        "application/vnd.google-apps.document",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    (
        "application/vnd.google-apps.spreadsheet",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (
        "application/vnd.google-apps.presentation",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("application/vnd.google-apps.drawing", "image/png"),
];

/// The type a document of `mime` is saved as, if it has one.
pub fn export_of(mime: &str) -> Option<String> {
    EXPORTS
        .iter()
        .find(|(native, _)| *native == mime)
        .map(|(_, office)| (*office).to_owned())
}

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
    #[serde(rename = "modifiedTime")]
    pub modified: Option<String>,
    #[serde(rename = "webViewLink")]
    pub web_view_link: Option<String>,
    #[serde(rename = "shortcutDetails")]
    pub shortcut: Option<ShortcutDetails>,
}

/// What a shortcut points at.
#[derive(Debug, Clone, Deserialize)]
pub struct ShortcutDetails {
    #[serde(rename = "targetId")]
    pub target_id: Option<String>,
    #[serde(rename = "targetMimeType")]
    pub target_mime: Option<String>,
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

    /// What it is in My Drive: a folder, a file with bytes, or a Google document or a shortcut,
    /// which open online (a shortcut where its target opens). `None` for an online item with no
    /// page to open.
    pub fn kind(&self) -> Option<ItemKind> {
        let mime = self.mime.as_deref().unwrap_or_default();
        if mime == FOLDER {
            return Some(ItemKind::Folder);
        }
        if mime == SHORTCUT {
            let target = self.shortcut.as_ref()?;
            let url = WebUrl::parse(&format!("{OPEN}{}", target.target_id.as_deref()?)).ok()?;
            let export = target.target_mime.as_deref().and_then(export_of);
            return Some(ItemKind::Online { url, export });
        }
        if !mime.starts_with(NATIVE) {
            return Some(ItemKind::File);
        }
        let url = match self.web_view_link.as_deref() {
            Some(link) => WebUrl::parse(link).ok()?,
            None => WebUrl::parse(&format!("{OPEN}{}", self.id)).ok()?,
        };
        Some(ItemKind::Online {
            url,
            export: export_of(mime),
        })
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

/// `files.generateIds`.
#[derive(Debug, Deserialize)]
pub struct GeneratedIds {
    #[serde(default)]
    pub ids: Vec<String>,
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

    #[test]
    fn a_google_document_opens_online_and_saves_as_its_office_type_and_a_shortcut_as_its_target() {
        let online = |url: &str, export: Option<&str>| {
            Some(ItemKind::Online {
                url: WebUrl::parse(url).expect("url"),
                export: export.map(str::to_owned),
            })
        };
        let docx = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
        let xlsx = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
        let pptx = "application/vnd.openxmlformats-officedocument.presentationml.presentation";
        let cases: Vec<(&str, &str, Option<ItemKind>)> = vec![
            (
                "doc",
                r#"{"id":"d","mimeType":"application/vnd.google-apps.document","webViewLink":"https://docs.example.test/document/d/d/edit"}"#,
                online("https://docs.example.test/document/d/d/edit", Some(docx)),
            ),
            (
                "sheet",
                r#"{"id":"s","mimeType":"application/vnd.google-apps.spreadsheet","webViewLink":"https://docs.example.test/s"}"#,
                online("https://docs.example.test/s", Some(xlsx)),
            ),
            (
                "slides",
                r#"{"id":"p","mimeType":"application/vnd.google-apps.presentation","webViewLink":"https://docs.example.test/p"}"#,
                online("https://docs.example.test/p", Some(pptx)),
            ),
            (
                "drawing",
                r#"{"id":"w","mimeType":"application/vnd.google-apps.drawing","webViewLink":"https://docs.example.test/w"}"#,
                online("https://docs.example.test/w", Some("image/png")),
            ),
            (
                "form",
                r#"{"id":"f","mimeType":"application/vnd.google-apps.form","webViewLink":"https://docs.example.test/f"}"#,
                online("https://docs.example.test/f", None),
            ),
            (
                "site with no page named",
                r#"{"id":"t","mimeType":"application/vnd.google-apps.site"}"#,
                online("https://drive.google.com/open?id=t", None),
            ),
            (
                "shortcut to a sheet",
                r#"{"id":"c","mimeType":"application/vnd.google-apps.shortcut","shortcutDetails":{"targetId":"s","targetMimeType":"application/vnd.google-apps.spreadsheet"}}"#,
                online("https://drive.google.com/open?id=s", Some(xlsx)),
            ),
            (
                "shortcut that names no target",
                r#"{"id":"c2","mimeType":"application/vnd.google-apps.shortcut"}"#,
                None,
            ),
            (
                "pdf",
                r#"{"id":"b","mimeType":"application/pdf"}"#,
                Some(ItemKind::File),
            ),
            (
                "folder",
                r#"{"id":"o","mimeType":"application/vnd.google-apps.folder"}"#,
                Some(ItemKind::Folder),
            ),
        ];
        for (name, json, want) in cases {
            let file: File = serde_json::from_str(json).expect("file");
            assert_eq!(file.kind(), want, "{name}");
        }
    }
}
