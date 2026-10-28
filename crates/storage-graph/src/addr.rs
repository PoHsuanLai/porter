//! Where things are: the drive's URLs for the dataset's folder, an item by id or by path, and
//! the check that a link the server handed back (a delta link) is still this server's. Paths
//! start at the root the replica was given: the app folder or the drive's own root.

use porter_ids::WebUrl;
use porter_ids::percent::encode_unreserved;
use porter_sync::ItemPath;

/// Graph's version prefix and the signed-in user's drive.
const DRIVE: &str = "/v1.0/me/drive";

/// Where a replica's paths start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Root {
    /// OneDrive's app folder (`special/approot`): what `Files.ReadWrite.AppFolder` reaches.
    #[default]
    AppFolder,
    /// The drive's own root (`root`): the whole drive, which `Files.ReadWrite` reaches. Its
    /// changes are read by a delta query of the root, which work and school drives answer too.
    Drive,
}

impl Root {
    /// The segment after `/me/drive` that names it.
    fn segment(self) -> &'static str {
        match self {
            Root::AppFolder => "special/approot",
            Root::Drive => "root",
        }
    }
}

/// The dataset's folder below the root, and the origin it is served from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addr {
    /// `scheme://authority`.
    origin: String,
    /// Where paths start.
    root: Root,
    /// The folder's names below the root; none is the root itself.
    folder: Vec<String>,
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
            root: Root::AppFolder,
            folder: folder
                .split('/')
                .filter(|n| !n.is_empty() && *n != "." && *n != "..")
                .map(str::to_owned)
                .collect(),
        }
    }

    /// The same folder below `root`.
    pub fn with_root(self, root: Root) -> Self {
        Self { root, ..self }
    }

    /// Where paths start.
    pub fn root(&self) -> Root {
        self.root
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
        self.under_root(&names, op, query)
    }

    /// Where the change feed starts: below the app folder the dataset folder's own delta, in a
    /// whole drive the root's (folder-scoped delta is answered by personal drives only).
    pub fn delta(&self) -> Option<WebUrl> {
        match self.root {
            Root::AppFolder => self.path(None, Some("delta"), ""),
            Root::Drive => self.under_root(&[], Some("delta"), ""),
        }
    }

    /// The item at `names` below the root, and `op` asked of it.
    pub fn under_root(&self, names: &[String], op: Option<&str>, query: &str) -> Option<WebUrl> {
        let base = format!("{}{DRIVE}/{}", self.origin, self.root.segment());
        let url = match (names.is_empty(), op) {
            (true, None) => base,
            (true, Some(op)) => format!("{base}/{op}"),
            (false, op) => {
                let path = names
                    .iter()
                    .map(|n| encode_unreserved(n))
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
            encode_unreserved(id)
        ))
        .ok()
    }

    /// The item `name` in the folder `parent` (`None`: the dataset's folder), and `op` asked of
    /// it: `/items/{parent}:/{name}` (`:/{op}`), or the same below the dataset's folder.
    pub fn child(
        &self,
        parent: Option<&str>,
        name: &str,
        op: Option<&str>,
        query: &str,
    ) -> Option<WebUrl> {
        match parent {
            None => self.path(Some(&[name.to_owned()]), op, query),
            Some(id) => WebUrl::parse(&format!(
                "{}{DRIVE}/items/{}:/{}{}{query}",
                self.origin,
                encode_unreserved(id),
                encode_unreserved(name),
                op.map_or_else(String::new, |op| format!(":/{op}"))
            ))
            .ok(),
        }
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

    /// The dataset's folder as names below the root.
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
            (
                "child of an id".into(),
                addr("").child(Some("P!1"), "a b.txt", Some("content"), "?x=1"),
            ),
            (
                "child of the folder".into(),
                addr("Photos").child(None, "a.txt", Some("createUploadSession"), ""),
            ),
        ];
        let want = [
            "https://graph.microsoft.com/v1.0/me/drive/special/approot",
            "https://graph.microsoft.com/v1.0/me/drive/special/approot/delta?token=5",
            "https://graph.microsoft.com/v1.0/me/drive/special/approot:/Photos/Originals",
            "https://graph.microsoft.com/v1.0/me/drive/special/approot:/Photos:/delta",
            "https://graph.microsoft.com/v1.0/me/drive/special/approot:/Photos/2026/a%20b%2Bc.jpg:/content",
            "https://graph.microsoft.com/v1.0/me/drive/items/01AB%21c%20d",
            "https://graph.microsoft.com/v1.0/me/drive/items/X/content",
            "https://graph.microsoft.com/v1.0/me/drive/items/P%211:/a%20b.txt:/content?x=1",
            "https://graph.microsoft.com/v1.0/me/drive/special/approot:/Photos/a.txt:/createUploadSession",
        ];
        for ((name, got), want) in cases.iter().zip(want) {
            assert_eq!(got.as_ref().map(WebUrl::as_str), Some(want), "{name}");
        }
    }

    #[test]
    fn below_the_drive_root_urls_name_the_root_and_its_delta_is_the_roots() {
        let drive = |folder: &str| addr(folder).with_root(Root::Drive);
        let names = |p: &str| names_of(p).expect("names");
        let got = [
            drive("").path(None, None, ""),
            drive("").delta(),
            drive("Photos").delta(),
            drive("Photos").path(None, Some("children"), ""),
            drive("").path(Some(&names("a b/c.txt")), Some("content"), ""),
            addr("Photos").delta(),
        ];
        let want = [
            "https://graph.microsoft.com/v1.0/me/drive/root",
            "https://graph.microsoft.com/v1.0/me/drive/root/delta",
            "https://graph.microsoft.com/v1.0/me/drive/root/delta",
            "https://graph.microsoft.com/v1.0/me/drive/root:/Photos:/children",
            "https://graph.microsoft.com/v1.0/me/drive/root:/a%20b/c.txt:/content",
            "https://graph.microsoft.com/v1.0/me/drive/special/approot:/Photos:/delta",
        ];
        for (got, want) in got.iter().zip(want) {
            assert_eq!(got.as_ref().map(WebUrl::as_str), Some(want));
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
