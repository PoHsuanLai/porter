//! Where things are: the drive's URLs for the dataset's folder, an item by id or by path, and
//! the check that a link the server handed back (a delta link) is still this server's.

use porter_core::WebUrl;
use porter_sync::ItemPath;

/// Graph's version prefix and the signed-in user's drive.
const DRIVE: &str = "/v1.0/me/drive";

/// The dataset's folder in the app folder, and the origin it is served from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addr {
    /// `scheme://authority`.
    origin: String,
    /// The folder's names below the app folder; none is the app folder itself.
    folder: Vec<String>,
}

/// A name as one URL path segment: everything but unreserved characters percent-encoded.
fn encode(name: &str) -> String {
    name.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
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

impl Addr {
    /// The folder `folder` (a path below the app folder, possibly empty) of the drive at `base`.
    /// A part of `folder` that is empty or `.`/`..` is dropped.
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

    /// The drive itself (quota).
    pub fn drive(&self) -> Option<WebUrl> {
        WebUrl::parse(&format!("{}{DRIVE}", self.origin)).ok()
    }

    /// The item at `rel` below the dataset's folder (the folder itself for `None`), and `op`
    /// asked of it (`content`, `delta`, `children`, `createUploadSession`).
    pub fn path(&self, rel: Option<&[String]>, op: Option<&str>, query: &str) -> Option<WebUrl> {
        let names: Vec<String> = self
            .folder
            .iter()
            .chain(rel.unwrap_or(&[]))
            .cloned()
            .collect();
        self.under_app(&names, op, query)
    }

    /// The item at `names` below the app folder, and `op` asked of it.
    pub fn under_app(&self, names: &[String], op: Option<&str>, query: &str) -> Option<WebUrl> {
        let base = format!("{}{DRIVE}/special/approot", self.origin);
        let url = match (names.is_empty(), op) {
            (true, None) => base,
            (true, Some(op)) => format!("{base}/{op}"),
            (false, op) => {
                let path = names
                    .iter()
                    .map(|n| encode(n))
                    .collect::<Vec<_>>()
                    .join("/");
                match op {
                    None => format!("{base}:/{path}"),
                    Some(op) => format!("{base}:/{path}:/{op}"),
                }
            }
        };
        WebUrl::parse(&format!("{url}{query}")).ok()
    }

    /// The item `id`, and `op` asked of it.
    pub fn item(&self, id: &str, op: Option<&str>, query: &str) -> Option<WebUrl> {
        let op = op.map_or_else(String::new, |op| format!("/{op}"));
        WebUrl::parse(&format!(
            "{}{DRIVE}/items/{}{op}{query}",
            self.origin,
            encode(id)
        ))
        .ok()
    }

    /// Whether `link` is a URL on this origin: a delta link from another server is not ours to
    /// follow.
    pub fn owns(&self, link: &str) -> bool {
        link.strip_prefix(&self.origin)
            .is_some_and(|rest| rest.starts_with('/'))
    }

    /// The parsed link, if it is ours.
    pub fn link(&self, link: &str) -> Option<WebUrl> {
        self.owns(link).then(|| WebUrl::parse(link).ok()).flatten()
    }

    /// The dataset's folder as names below the app folder.
    pub fn folder(&self) -> &[String] {
        &self.folder
    }
}

/// An item's path in the form the dataset names it.
pub fn item_path(names: &[String]) -> ItemPath {
    ItemPath(names.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(folder: &str) -> Addr {
        Addr::new(
            &WebUrl::parse("https://graph.microsoft.com").expect("url"),
            folder,
        )
    }

    #[test]
    fn urls_name_the_app_folder_a_path_or_an_id_and_encode_what_a_name_may_hold() {
        let names = |p: &str| names_of(p).expect("names");
        let cases: Vec<(String, Option<WebUrl>)> = vec![
            ("app".into(), addr("").path(None, None, "")),
            (
                "app delta".into(),
                addr("").path(None, Some("delta"), "?token=5"),
            ),
            (
                "folder".into(),
                addr("Photos/Originals").path(None, None, ""),
            ),
            (
                "folder delta".into(),
                addr("Photos").path(None, Some("delta"), ""),
            ),
            (
                "file content".into(),
                addr("Photos").path(Some(&names("2026/a b+c.jpg")), Some("content"), ""),
            ),
            ("id".into(), addr("").item("01AB!c d", None, "")),
            ("id content".into(), addr("").item("X", Some("content"), "")),
        ];
        let want = [
            "https://graph.microsoft.com/v1.0/me/drive/special/approot",
            "https://graph.microsoft.com/v1.0/me/drive/special/approot/delta?token=5",
            "https://graph.microsoft.com/v1.0/me/drive/special/approot:/Photos/Originals",
            "https://graph.microsoft.com/v1.0/me/drive/special/approot:/Photos:/delta",
            "https://graph.microsoft.com/v1.0/me/drive/special/approot:/Photos/2026/a%20b%2Bc.jpg:/content",
            "https://graph.microsoft.com/v1.0/me/drive/items/01AB%21c%20d",
            "https://graph.microsoft.com/v1.0/me/drive/items/X/content",
        ];
        for ((name, got), want) in cases.iter().zip(want) {
            assert_eq!(got.as_ref().map(WebUrl::as_str), Some(want), "{name}");
        }
    }

    #[test]
    fn a_path_stays_inside_the_folder() {
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

    #[test]
    fn a_link_is_followed_only_on_the_origin_that_made_it() {
        let a = addr("");
        const CASES: &[(&str, bool)] = &[
            (
                "https://graph.microsoft.com/v1.0/me/drive/root/delta?token=1",
                true,
            ),
            ("https://graph.microsoft.com.evil.example/v1.0/x", false),
            ("https://graph.microsoft.com", false),
            ("http://graph.microsoft.com/v1.0/x", false),
            ("https://other.example/v1.0/x", false),
        ];
        for (link, ok) in CASES {
            assert_eq!(a.owns(link), *ok, "{link}");
        }
    }
}
