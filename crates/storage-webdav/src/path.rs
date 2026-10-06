//! Paths as the replica names things: a canonical percent-encoded absolute path is the
//! [`RemoteId`], the decoded path under the dataset folder is the [`ItemPath`].
//!
//! Servers spell one resource several ways (an absolute URL or a path, `%2f` or `%2F`, a
//! trailing slash on a folder), so every href is decoded and encoded again the one way before it
//! is compared or kept.

use porter_core::WebUrl;
use porter_sync::{ItemPath, RemoteId};

/// The dataset's folder on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// `scheme://authority` as the folder's URL wrote it: what `Host` is made from.
    origin: String,
    /// The folder's canonical path, with its trailing slash.
    path: String,
}

impl Root {
    /// The folder at `url`.
    pub fn new(url: &WebUrl) -> Self {
        let text = url.as_str();
        let after = text.find("://").map_or(0, |at| at + 3);
        let end = text[after..]
            .find(['/', '?'])
            .map_or(text.len(), |at| after + at);
        Self {
            origin: text[..end].to_owned(),
            path: format!("{}/", canonical(url.path())),
        }
    }

    /// The folder's id: its canonical path, without a trailing slash.
    pub fn folder(&self) -> RemoteId {
        RemoteId(self.path.trim_end_matches('/').to_owned())
    }

    /// The URL of the resource `id` names.
    pub fn url_of(&self, id: &RemoteId) -> Option<WebUrl> {
        WebUrl::parse(&format!("{}{}", self.origin, id.0)).ok()
    }

    /// The URL of the folder at `id`, with the trailing slash a collection's URL carries.
    pub fn collection_url(&self, id: &RemoteId) -> Option<WebUrl> {
        WebUrl::parse(&format!("{}{}/", self.origin, id.0)).ok()
    }

    /// The URL of the dataset's folder.
    pub fn folder_url(&self) -> Option<WebUrl> {
        WebUrl::parse(&format!("{}{}", self.origin, self.path)).ok()
    }

    /// The id of the resource an href names, if it is inside the folder.
    pub fn id_of(&self, href: &str) -> Option<RemoteId> {
        let id = RemoteId(canonical(path_of(href)));
        let inside = id.0.starts_with(&self.path) || id == self.folder();
        inside.then_some(id)
    }

    /// The decoded path of `id` under the folder; `None` for the folder itself.
    pub fn item_path(&self, id: &RemoteId) -> Option<ItemPath> {
        let rest = id.0.strip_prefix(&self.path)?;
        (!rest.is_empty()).then(|| ItemPath(decode(rest)))
    }

    /// The id a new item at `path` would have, if `path` stays inside the folder: no empty,
    /// `.` or `..` segment, no leading slash.
    pub fn id_for(&self, path: &ItemPath) -> Option<RemoteId> {
        let clean = !path.0.is_empty()
            && path
                .0
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..");
        clean.then(|| RemoteId(format!("{}{}", self.path, encode(&path.0))))
    }
}

/// The folder ids between the dataset's folder and `id`, outermost first (what a PUT may need
/// to create): `a/b/c.jpg` has `a` and `a/b`.
pub fn parents_of(root: &Root, id: &RemoteId) -> Vec<RemoteId> {
    let Some(rest) = id.0.strip_prefix(&root.path) else {
        return Vec::new();
    };
    let parts: Vec<&str> = rest.split('/').collect();
    (1..parts.len())
        .map(|n| RemoteId(format!("{}{}", root.path, parts[..n].join("/"))))
        .collect()
}

/// An href's path: an absolute URL loses its scheme and authority, any query is dropped.
fn path_of(href: &str) -> &str {
    let rest = match href.split_once("://") {
        Some((_, after)) => after.find('/').map_or("/", |at| &after[at..]),
        None => href,
    };
    rest.split(['?', '#']).next().unwrap_or(rest)
}

/// The one spelling of a path: decoded, encoded again, no trailing slash (the root stays `/`).
fn canonical(path: &str) -> String {
    let encoded = encode(&decode(path));
    match encoded.trim_end_matches('/') {
        "" => String::new(),
        trimmed => trimmed.to_owned(),
    }
}

/// Percent-encodes everything but unreserved characters and `/`.
pub fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Percent-decodes; a stray `%` stays as written.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let hex = |b: u8| char::from(b).to_digit(16);
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let pair = bytes
            .get(at + 1)
            .zip(bytes.get(at + 2))
            .and_then(|(hi, lo)| Some(u8::try_from(hex(*hi)? * 16 + hex(*lo)?).ok()?));
        match (bytes[at], pair) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> Root {
        Root::new(
            &WebUrl::parse("http://127.0.0.1:8080/remote.php/dav/files/ada/Photos%20/")
                .expect("url"),
        )
    }

    #[test]
    fn an_href_has_one_spelling_whatever_the_server_wrote() {
        let root = root();
        let want = RemoteId("/remote.php/dav/files/ada/Photos%20/a%20b.jpg".into());
        const CASES: &[&str] = &[
            "/remote.php/dav/files/ada/Photos%20/a%20b.jpg",
            "/remote.php/dav/files/ada/Photos /a b.jpg",
            "http://127.0.0.1:8080/remote.php/dav/files/ada/Photos%20/a%20b.jpg",
            "/remote.php/dav/files/ada/Photos%20/a%20b.jpg?x=1",
            "/remote.php/dav/files/ada/Photos%20/a%20b%2ejpg",
        ];
        for href in CASES {
            assert_eq!(root.id_of(href).as_ref(), Some(&want), "{href}");
        }
    }

    #[test]
    fn the_folder_and_what_is_outside_it() {
        let root = root();
        assert_eq!(
            root.id_of("/remote.php/dav/files/ada/Photos%20/"),
            Some(root.folder())
        );
        assert_eq!(root.id_of("/remote.php/dav/files/ada/Photos%20x/a"), None);
        assert_eq!(root.id_of("/remote.php/dav/files/ada/"), None);
        assert_eq!(root.item_path(&root.folder()), None);
    }

    #[test]
    fn a_dataset_path_maps_to_an_id_and_back_and_may_not_climb_out() {
        let root = root();
        const CASES: &[(&str, Option<&str>)] = &[
            (
                "Originals/2026/IMG 1.HEIC",
                Some("/remote.php/dav/files/ada/Photos%20/Originals/2026/IMG%201.HEIC"),
            ),
            ("a.jpg", Some("/remote.php/dav/files/ada/Photos%20/a.jpg")),
            ("", None),
            ("/abs", None),
            ("a//b", None),
            ("../x", None),
            ("a/./b", None),
            ("a/", None),
        ];
        for (path, id) in CASES {
            let got = root.id_for(&ItemPath((*path).into()));
            assert_eq!(got.as_ref().map(|i| i.0.as_str()), *id, "{path:?}");
            if let Some(got) = got {
                assert_eq!(root.item_path(&got), Some(ItemPath((*path).into())));
            }
        }
    }

    #[test]
    fn the_parents_of_a_nested_item_are_listed_outermost_first() {
        let root = root();
        let id = root.id_for(&ItemPath("a/b/c.jpg".into())).expect("id");
        let parents: Vec<String> = parents_of(&root, &id).into_iter().map(|p| p.0).collect();
        assert_eq!(
            parents,
            [
                "/remote.php/dav/files/ada/Photos%20/a",
                "/remote.php/dav/files/ada/Photos%20/a/b"
            ]
        );
        assert!(parents_of(&root, &root.id_for(&ItemPath("c.jpg".into())).expect("id")).is_empty());
    }

    #[test]
    fn encoding_round_trips_and_a_stray_percent_survives() {
        for text in ["a b/ü.jpg", "100%", "x%zz", "plain/path-1_2.~"] {
            assert_eq!(decode(&encode(text)), text);
        }
        assert_eq!(decode("100%"), "100%");
    }
}
