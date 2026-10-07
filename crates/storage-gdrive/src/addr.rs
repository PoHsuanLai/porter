//! Where things are: the Drive API's URLs on the origin the replica is handed, the query strings
//! they take, and the dataset's folder below the app data folder.

use porter_core::{Origin, WebUrl};
use porter_sync::ItemPath;

/// The alias Drive gives the app data folder's id.
pub const ROOT_ALIAS: &str = "appDataFolder";

/// The fields every file answer is asked for.
pub const FILE_FIELDS: &str = "id,name,mimeType,parents,version,md5Checksum,size,trashed";

/// A value as a URL query component: everything but unreserved characters percent-encoded.
pub fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `?k=v&k=v` (nothing for no pairs).
pub fn query(pairs: &[(&str, &str)]) -> String {
    match pairs.is_empty() {
        true => String::new(),
        false => format!(
            "?{}",
            pairs
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect::<Vec<_>>()
                .join("&")
        ),
    }
}

/// A name inside a `q` string literal: backslash and quote escaped.
pub fn literal(name: &str) -> String {
    format!("'{}'", name.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The names a relative path is made of, or `None` if it is empty, has an empty, `.` or `..`
/// part, or starts with a slash.
pub fn names_of(path: &str) -> Option<Vec<String>> {
    let names: Vec<String> = path.split('/').map(str::to_owned).collect();
    names
        .iter()
        .all(|n| !n.is_empty() && n != "." && n != "..")
        .then_some(names)
}

/// An item's path in the form the dataset names it.
pub fn item_path(names: &[String]) -> ItemPath {
    ItemPath(names.join("/"))
}

/// The dataset's folder in the app data folder, and the origin the API is served from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addr {
    /// `scheme://authority`.
    origin: String,
    /// The folder's names below the app data folder; none is the app data folder itself.
    folder: Vec<String>,
}

impl Addr {
    /// The folder `folder` (a path below the app data folder, possibly empty) of the API at
    /// `base` (`https://www.googleapis.com/drive/v3`: only its origin is used). A part of
    /// `folder` that is empty or `.`/`..` is dropped.
    pub fn new(base: &WebUrl, folder: &str) -> Self {
        let text = base.as_str();
        let after = text.find("://").map_or(0, |at| at + 3);
        let end = text[after..]
            .find(['/', '?'])
            .map_or(text.len(), |at| after + at);
        Self {
            origin: text[..end].to_owned(),
            folder: folder
                .split('/')
                .filter(|n| !n.is_empty() && *n != "." && *n != "..")
                .map(str::to_owned)
                .collect(),
        }
    }

    /// The API's own origin: a resumable session's URL must be on it.
    pub fn origin(&self) -> Option<Origin> {
        WebUrl::parse(&self.origin).ok().map(|u| u.origin())
    }

    /// `/drive/v3/<path>` with a query.
    pub fn api(&self, path: &str, pairs: &[(&str, &str)]) -> Option<WebUrl> {
        WebUrl::parse(&format!("{}/drive/v3/{path}{}", self.origin, query(pairs))).ok()
    }

    /// `/upload/drive/v3/<path>` with a query.
    pub fn upload(&self, path: &str, pairs: &[(&str, &str)]) -> Option<WebUrl> {
        WebUrl::parse(&format!(
            "{}/upload/drive/v3/{path}{}",
            self.origin,
            query(pairs)
        ))
        .ok()
    }

    /// The file `id`.
    pub fn file(&self, id: &str, pairs: &[(&str, &str)]) -> Option<WebUrl> {
        self.api(&format!("files/{}", encode(id)), pairs)
    }

    /// The dataset's folder as names below the app data folder.
    pub fn folder(&self) -> &[String] {
        &self.folder
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr() -> Addr {
        Addr::new(
            &WebUrl::parse("https://www.googleapis.com/drive/v3").expect("url"),
            "Photos/Originals",
        )
    }

    #[test]
    fn urls_name_the_api_the_upload_host_and_a_file_and_encode_what_a_value_may_hold() {
        let a = addr();
        let got = [
            a.api("about", &[("fields", "storageQuota")]),
            a.upload("files", &[("uploadType", "multipart")]),
            a.file("1a b", &[("alt", "media")]),
            a.api("files", &[("q", "name = 'x' and trashed = false")]),
            a.api("changes/startPageToken", &[]),
        ];
        let want = [
            "https://www.googleapis.com/drive/v3/about?fields=storageQuota",
            "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart",
            "https://www.googleapis.com/drive/v3/files/1a%20b?alt=media",
            "https://www.googleapis.com/drive/v3/files?q=name%20%3D%20%27x%27%20and%20trashed%20%3D%20false",
            "https://www.googleapis.com/drive/v3/changes/startPageToken",
        ];
        for (got, want) in got.iter().zip(want) {
            assert_eq!(got.as_ref().map(WebUrl::as_str), Some(want));
        }
        assert_eq!(a.folder(), ["Photos", "Originals"]);
    }

    #[test]
    fn a_name_in_a_query_is_escaped_and_a_path_stays_inside_the_folder() {
        assert_eq!(literal("it's a \\ name"), r"'it\'s a \\ name'");
        const CASES: &[(&str, bool)] = &[
            ("a.jpg", true),
            ("2026/a.jpg", true),
            ("", false),
            ("/a.jpg", false),
            ("a//b", false),
            ("../a", false),
            ("a/./b", false),
            ("a/", false),
        ];
        for (path, ok) in CASES {
            assert_eq!(names_of(path).is_some(), *ok, "{path:?}");
        }
    }
}
