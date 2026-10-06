//! The DAV tree the Nextcloud and plain-DAV fakes serve: files, calendars and address books as
//! one path space with a change counter, so PROPFIND, a `sync-collection` REPORT with a sync
//! token, quota properties, PUT, DELETE and MKCOL all agree.

use crate::http::{Request, Response};
use std::collections::BTreeMap;

/// What a node is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A folder of files (quota properties live here).
    Files,
    /// A calendar of this component (`VEVENT`, `VTODO`).
    Calendar(String),
    /// An address book.
    AddressBook,
    /// A plain collection with no special properties.
    Container,
    /// A file, event, task or contact.
    Item,
}

#[derive(Debug, Clone)]
struct Node {
    kind: Kind,
    body: Vec<u8>,
    content_type: String,
    id: u64,
    changed: u64,
}

/// How much quota the server reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    /// Bytes counted as used before any file here.
    pub used_base: u64,
    /// Bytes reported available; `-3` is Nextcloud's "unlimited".
    pub available: i64,
}

/// The tree and its change log.
#[derive(Debug, Clone)]
pub struct Tree {
    nodes: BTreeMap<String, Node>,
    deleted: Vec<(String, u64)>,
    seq: u64,
    oldest_valid: u64,
    quota: Quota,
    limit: Option<u64>,
    behaviour: Behaviour,
}

/// How this server differs from the plain one: knobs a replica test turns to meet servers that
/// answer unlike each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Behaviour {
    /// Whether a change to a file changes the etag of every folder above it (Nextcloud's files).
    pub etag_propagation: Propagation,
    /// Whether the server answers the `sync-collection` REPORT (Nextcloud's files do not).
    pub sync_collection: SyncCollection,
}

/// Whether folder etags follow their contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Propagation {
    /// A folder's etag changes when anything below it does.
    Up,
    /// A folder's etag is its own.
    Off,
}

/// Whether the REPORT is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncCollection {
    /// Served, at `sync-level` 1 and `infinite`.
    Served,
    /// Answered `501 Not Implemented`.
    NotImplemented,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            etag_propagation: Propagation::Off,
            sync_collection: SyncCollection::Served,
        }
    }
}

const NAMESPACES: &str = "xmlns:d=\"DAV:\" xmlns:s=\"http://sabredav.org/ns\" xmlns:oc=\"http://owncloud.org/ns\" \
    xmlns:nc=\"http://nextcloud.org/ns\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\" \
    xmlns:card=\"urn:ietf:params:xml:ns:carddav\" xmlns:cs=\"http://calendarserver.org/ns/\"";

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(p, _)| p)
}

fn etag(changed: u64) -> String {
    format!("\"{changed:x}\"")
}

fn token_url(seq: u64) -> String {
    format!("http://fake.test/ns/sync/{seq}")
}

impl Tree {
    /// An empty tree with these collections.
    pub fn with_collections(collections: &[(&str, Kind)]) -> Self {
        let mut tree = Self {
            nodes: BTreeMap::new(),
            deleted: Vec::new(),
            seq: 1,
            oldest_valid: 0,
            quota: Quota {
                used_base: 0,
                available: -3,
            },
            limit: None,
            behaviour: Behaviour::default(),
        };
        for (path, kind) in collections {
            tree.insert(path, kind.clone(), Vec::new(), "httpd/unix-directory");
        }
        tree
    }

    /// Nextcloud's layout for `user`: files, a calendar, a task list and an address book.
    pub fn nextcloud(user: &str) -> Self {
        let dav = "/remote.php/dav";
        let paths = [
            (dav.to_owned(), Kind::Container),
            (format!("{dav}/files"), Kind::Container),
            (format!("{dav}/files/{user}"), Kind::Files),
            (format!("{dav}/calendars"), Kind::Container),
            (format!("{dav}/calendars/{user}"), Kind::Container),
            (
                format!("{dav}/calendars/{user}/personal"),
                Kind::Calendar("VEVENT".to_owned()),
            ),
            (
                format!("{dav}/calendars/{user}/tasks"),
                Kind::Calendar("VTODO".to_owned()),
            ),
            (format!("{dav}/addressbooks"), Kind::Container),
            (format!("{dav}/addressbooks/users"), Kind::Container),
            (format!("{dav}/addressbooks/users/{user}"), Kind::Container),
            (
                format!("{dav}/addressbooks/users/{user}/contacts"),
                Kind::AddressBook,
            ),
        ];
        let borrowed: Vec<(&str, Kind)> =
            paths.iter().map(|(p, k)| (p.as_str(), k.clone())).collect();
        Self::with_collections(&borrowed)
    }

    fn insert(&mut self, path: &str, kind: Kind, body: Vec<u8>, content_type: &str) {
        self.seq += 1;
        let node = Node {
            kind,
            body,
            content_type: content_type.to_owned(),
            id: self.seq,
            changed: self.seq,
        };
        self.nodes.insert(path.to_owned(), node);
        self.touch_above(path);
    }

    /// Under etag propagation, the folders above `path` change with it.
    fn touch_above(&mut self, path: &str) {
        if self.behaviour.etag_propagation == Propagation::Off {
            return;
        }
        let seq = self.seq;
        let mut up = parent_of(path).to_owned();
        while let Some(node) = self.nodes.get_mut(&up) {
            node.changed = seq;
            up = parent_of(&up).to_owned();
        }
    }

    /// Creates or replaces the item at `path` (its parent collection must exist).
    pub fn put(&mut self, path: &str, body: &[u8]) -> Result<PutOutcome, PutError> {
        let parent = self.nodes.get(parent_of(path)).ok_or(PutError::NoParent)?;
        if parent.kind == Kind::Item {
            return Err(PutError::NoParent);
        }
        let content_type = match parent.kind {
            Kind::Calendar(_) => "text/calendar",
            Kind::AddressBook => "text/vcard",
            _ => "application/octet-stream",
        };
        let outcome = match self.nodes.contains_key(path) {
            true => PutOutcome::Replaced,
            false => PutOutcome::Created,
        };
        self.insert(path, Kind::Item, body.to_vec(), content_type);
        Ok(outcome)
    }

    /// Makes a collection (inside a files folder it is a files folder).
    pub fn mkcol(&mut self, path: &str) -> Result<(), PutError> {
        if self.nodes.contains_key(path) {
            return Err(PutError::Exists);
        }
        let parent = self.nodes.get(parent_of(path)).ok_or(PutError::NoParent)?;
        let kind = if parent.kind == Kind::Files {
            Kind::Files
        } else {
            Kind::Container
        };
        self.insert(path, kind, Vec::new(), "httpd/unix-directory");
        Ok(())
    }

    /// Removes `path` and everything under it; whether it existed.
    pub fn delete(&mut self, path: &str) -> bool {
        let under = format!("{path}/");
        let gone: Vec<String> = self
            .nodes
            .keys()
            .filter(|k| *k == path || k.starts_with(&under))
            .cloned()
            .collect();
        self.seq += 1;
        for key in &gone {
            self.nodes.remove(key);
            self.deleted.push((key.clone(), self.seq));
        }
        if !gone.is_empty() {
            self.touch_above(path);
        }
        !gone.is_empty()
    }

    /// The body at `path`, for a GET.
    pub fn body(&self, path: &str) -> Option<(&[u8], &str)> {
        let node = self.nodes.get(path).filter(|n| n.kind == Kind::Item)?;
        Some((&node.body, &node.content_type))
    }

    /// Sets what quota reports.
    pub fn set_quota(&mut self, quota: Quota) {
        self.quota = quota;
    }

    /// Gives the account room for `total` bytes: a PUT past it is `507`, and the quota
    /// properties report what is left.
    pub fn set_limit(&mut self, total: u64) {
        self.limit = Some(total);
    }

    /// How this server differs from the plain one.
    pub fn set_behaviour(&mut self, behaviour: Behaviour) {
        self.behaviour = behaviour;
    }

    /// The etag of the node at `path`, quoted as the server sends it.
    pub fn etag_of(&self, path: &str) -> Option<String> {
        self.nodes.get(path).map(|n| etag(n.changed))
    }

    /// The sync token a client would get now.
    pub fn sync_token(&self) -> String {
        token_url(self.seq)
    }

    /// Every sync token handed out so far stops being valid (the server pruned its change log).
    pub fn expire_sync_tokens(&mut self) {
        self.oldest_valid = self.seq;
    }

    fn children<'a>(&'a self, path: &'a str) -> impl Iterator<Item = (&'a String, &'a Node)> {
        self.nodes
            .iter()
            .filter(move |(k, _)| k.as_str() != path && parent_of(k) == path)
    }

    fn used_under(&self, path: &str) -> u64 {
        let under = format!("{path}/");
        let files: u64 = self
            .nodes
            .iter()
            .filter(|(k, n)| n.kind == Kind::Item && k.starts_with(&under))
            .map(|(_, n)| n.body.len() as u64)
            .sum();
        self.quota.used_base + files
    }

    /// Bytes of every file in any files folder, plus what the server counts before them.
    fn used_total(&self) -> u64 {
        let files: u64 = self
            .nodes
            .iter()
            .filter(|(k, n)| n.kind == Kind::Item && self.in_files(k))
            .map(|(_, n)| n.body.len() as u64)
            .sum();
        self.quota.used_base + files
    }

    fn in_files(&self, path: &str) -> bool {
        let mut up = parent_of(path);
        while let Some(node) = self.nodes.get(up) {
            if node.kind == Kind::Files {
                return true;
            }
            up = parent_of(up);
        }
        false
    }

    fn available(&self) -> i64 {
        match self.limit {
            Some(total) => {
                i64::try_from(total.saturating_sub(self.used_total())).unwrap_or(i64::MAX)
            }
            None => self.quota.available,
        }
    }

    fn props(&self, path: &str, node: &Node, requested: &str) -> String {
        let wants = |name: &str| {
            requested.trim().is_empty() || requested.contains("allprop") || requested.contains(name)
        };
        let mut out = String::new();
        let collection = node.kind != Kind::Item;
        if wants("resourcetype") {
            let extra = match &node.kind {
                Kind::Calendar(_) => "<cal:calendar/>",
                Kind::AddressBook => "<card:addressbook/>",
                _ => "",
            };
            out.push_str(&match collection {
                true => format!("<d:resourcetype><d:collection/>{extra}</d:resourcetype>"),
                false => "<d:resourcetype/>".to_owned(),
            });
        }
        if wants("getetag") {
            out.push_str(&format!(
                "<d:getetag>&quot;{:x}&quot;</d:getetag>",
                node.changed
            ));
        }
        if wants("getlastmodified") {
            out.push_str("<d:getlastmodified>Mon, 05 Oct 2026 12:00:00 GMT</d:getlastmodified>");
        }
        if wants("fileid") {
            out.push_str(&format!("<oc:fileid>{}</oc:fileid>", node.id));
        }
        if collection {
            let name = path.rsplit('/').next().unwrap_or("");
            out.push_str(&if wants("displayname") {
                format!("<d:displayname>{}</d:displayname>", escape(name))
            } else {
                String::new()
            });
            if wants("sync-token") {
                out.push_str(&format!(
                    "<d:sync-token>{}</d:sync-token>",
                    self.sync_token()
                ));
            }
            if matches!(node.kind, Kind::Calendar(_) | Kind::AddressBook) && wants("getctag") {
                out.push_str(&format!("<cs:getctag>{}</cs:getctag>", self.sync_token()));
            }
            if let (Kind::Calendar(component), true) =
                (&node.kind, wants("supported-calendar-component-set"))
            {
                out.push_str(&format!(
                    "<cal:supported-calendar-component-set><cal:comp name=\"{component}\"/></cal:supported-calendar-component-set>"
                ));
            }
            if node.kind == Kind::Files {
                if wants("quota-used-bytes") {
                    out.push_str(&format!(
                        "<d:quota-used-bytes>{}</d:quota-used-bytes>",
                        self.used_under(path)
                    ));
                }
                if wants("quota-available-bytes") {
                    out.push_str(&format!(
                        "<d:quota-available-bytes>{}</d:quota-available-bytes>",
                        self.available()
                    ));
                }
            }
        } else {
            if wants("getcontentlength") {
                out.push_str(&format!(
                    "<d:getcontentlength>{}</d:getcontentlength>",
                    node.body.len()
                ));
            }
            if wants("getcontenttype") {
                out.push_str(&format!(
                    "<d:getcontenttype>{}</d:getcontenttype>",
                    node.content_type
                ));
            }
        }
        out
    }

    fn response_for(&self, path: &str, node: &Node, requested: &str) -> String {
        let slash = if node.kind == Kind::Item { "" } else { "/" };
        format!(
            "<d:response><d:href>{}{slash}</d:href><d:propstat><d:prop>{}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
            escape(path),
            self.props(path, node, requested)
        )
    }

    fn multistatus(responses: &str, trailer: &str) -> Response {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><d:multistatus {NAMESPACES}>{responses}{trailer}</d:multistatus>"
        );
        Response::new(207).typed("application/xml; charset=utf-8", body)
    }

    /// PROPFIND at `path` with `Depth` `depth` (`0`, `1`; anything else reads as `1`).
    pub fn propfind(&self, path: &str, depth: &str, requested: &str) -> Response {
        let Some(node) = self.nodes.get(path) else {
            return Response::new(404);
        };
        let mut out = self.response_for(path, node, requested);
        if depth != "0" {
            for (child_path, child) in self.children(path) {
                out.push_str(&self.response_for(child_path, child, requested));
            }
        }
        Self::multistatus(&out, "")
    }

    /// The `sync-collection` REPORT: what changed under `path` since the token in `body`, among
    /// its members (`sync-level` 1) or everything below it (`infinite`).
    pub fn report(&self, path: &str, body: &str) -> Response {
        if self.behaviour.sync_collection == SyncCollection::NotImplemented {
            return Response::new(501);
        }
        if self.nodes.get(path).is_none_or(|n| n.kind == Kind::Item) {
            return Response::new(404);
        }
        if !body.contains("sync-collection") {
            return Response::new(400).typed("text/plain", "only sync-collection is supported");
        }
        let token = element_text(body, "sync-token").unwrap_or_default();
        let since = match token.trim() {
            "" => Some(0),
            t => t
                .strip_prefix("http://fake.test/ns/sync/")
                .and_then(|n| n.parse::<u64>().ok()),
        };
        // The first listing (no token) is always good; a token must not predate the pruning.
        let valid = match token.trim().is_empty() {
            true => Some(0),
            false => since.filter(|s| *s >= self.oldest_valid),
        };
        let Some(since) = valid else {
            let error =
                "<?xml version=\"1.0\"?><d:error xmlns:d=\"DAV:\"><d:valid-sync-token/></d:error>";
            return Response::new(403).typed("application/xml; charset=utf-8", error);
        };
        let infinite = element_text(body, "sync-level").is_some_and(|l| l.trim() == "infinite");
        let below = format!("{path}/");
        let within = |p: &str| match infinite {
            true => p.starts_with(&below),
            false => parent_of(p) == path,
        };
        let mut out = String::new();
        for (child_path, child) in self
            .nodes
            .iter()
            .filter(|(k, n)| within(k) && n.changed > since)
        {
            out.push_str(&self.response_for(child_path, child, body));
        }
        for (gone, _) in self
            .deleted
            .iter()
            .filter(|(p, at)| *at > since && within(p))
        {
            if !self.nodes.contains_key(gone) {
                out.push_str(&format!(
                    "<d:response><d:href>{}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>",
                    escape(gone)
                ));
            }
        }
        Self::multistatus(
            &out,
            &format!("<d:sync-token>{}</d:sync-token>", self.sync_token()),
        )
    }

    /// `412` when the request's `If-Match` or `If-None-Match` does not hold for the node at
    /// `path` (RFC 9110 section 13.1; an etag compares as the server wrote it, weak or not).
    fn precondition_failed(&self, request: &Request, path: &str) -> bool {
        let current = self
            .nodes
            .get(path)
            .filter(|n| n.kind == Kind::Item)
            .map(|n| etag(n.changed));
        let listed = |value: &str| -> Vec<String> {
            value
                .split(',')
                .map(|t| t.trim().trim_start_matches("W/").to_owned())
                .collect()
        };
        let if_match = request.header("if-match").map(listed);
        let if_none_match = request.header("if-none-match").map(listed);
        let match_fails = if_match.is_some_and(|tags| match &current {
            None => true,
            Some(now) => !tags.iter().any(|t| t == "*" || t == now),
        });
        let none_match_fails = if_none_match.is_some_and(|tags| match &current {
            None => false,
            Some(now) => tags.iter().any(|t| t == "*" || t == now),
        });
        match_fails || none_match_fails
    }

    /// A GET, with the `Range` header honoured (`bytes=a-b`, `bytes=a-`).
    fn get(&self, request: &Request, path: &str) -> Response {
        let Some(node) = self.nodes.get(path).filter(|n| n.kind == Kind::Item) else {
            return Response::new(404);
        };
        let whole = Response::new(200)
            .with_header("ETag", &etag(node.changed))
            .with_header("Accept-Ranges", "bytes")
            .typed(&node.content_type, node.body.clone());
        let Some(spec) = request
            .header("range")
            .and_then(|r| r.strip_prefix("bytes="))
        else {
            return whole;
        };
        let (from, to) = spec.split_once('-').unwrap_or((spec, ""));
        let len = node.body.len() as u64;
        let Ok(start) = from.parse::<u64>() else {
            return whole;
        };
        if start >= len {
            return Response::new(416).with_header("Content-Range", &format!("bytes */{len}"));
        }
        let end = to.parse::<u64>().map_or(len - 1, |e| e.min(len - 1));
        let slice = node.body[start as usize..=end as usize].to_vec();
        Response::new(206)
            .with_header("ETag", &etag(node.changed))
            .with_header("Content-Range", &format!("bytes {start}-{end}/{len}"))
            .typed(&node.content_type, slice)
    }

    /// A PUT with preconditions, quota and the new etag.
    fn put_request(&mut self, request: &Request, path: &str) -> Response {
        if self.precondition_failed(request, path) {
            return Response::new(412);
        }
        if let Some(total) = self.limit {
            let old = self
                .nodes
                .get(path)
                .filter(|n| n.kind == Kind::Item)
                .map_or(0, |n| n.body.len() as u64);
            let after = self.used_total().saturating_sub(old) + request.body.len() as u64;
            if after > total {
                return Response::new(507);
            }
        }
        match self.put(path, &request.body) {
            Ok(outcome) => {
                let status = match outcome {
                    PutOutcome::Created => 201,
                    PutOutcome::Replaced => 204,
                };
                let tag = self.etag_of(path).unwrap_or_default();
                Response::new(status).with_header("ETag", &tag)
            }
            Err(PutError::NoParent) => Response::new(409),
            Err(PutError::Exists) => Response::new(405),
        }
    }

    /// Answers a DAV request at `path`.
    pub fn handle(&mut self, request: &Request, path: &str) -> Response {
        let created = |outcome: Result<PutOutcome, PutError>| match outcome {
            Ok(PutOutcome::Created) => Response::new(201),
            Ok(PutOutcome::Replaced) => Response::new(204),
            Err(PutError::NoParent) => Response::new(409),
            Err(PutError::Exists) => Response::new(405),
        };
        match request.method.as_str() {
            "PROPFIND" => self.propfind(
                path,
                request.header("depth").unwrap_or("1"),
                &request.body_text(),
            ),
            "REPORT" => self.report(path, &request.body_text()),
            "PUT" => self.put_request(request, path),
            "MKCOL" => created(self.mkcol(path).map(|()| PutOutcome::Created)),
            "DELETE" if self.precondition_failed(request, path) => Response::new(412),
            "DELETE" => match self.delete(path) {
                true => Response::new(204),
                false => Response::new(404),
            },
            "GET" => self.get(request, path),
            _ => Response::new(405),
        }
    }
}

/// What a PUT did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    /// A new item.
    Created,
    /// An existing item replaced.
    Replaced,
}

/// Why a PUT or MKCOL did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutError {
    /// The parent collection does not exist.
    NoParent,
    /// The collection already exists.
    Exists,
}

/// The text of the first element named `name` (any prefix) in `xml`.
pub fn element_text(xml: &str, name: &str) -> Option<String> {
    let start = xml
        .find(&format!(":{name}>"))
        .or_else(|| xml.find(&format!("<{name}>")))?;
    let after = &xml[start..];
    let text_start = after.find('>')? + 1;
    let text_end = after[text_start..].find('<')?;
    Some(after[text_start..text_start + text_end].to_owned())
}
