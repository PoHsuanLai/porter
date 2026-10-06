//! The change feed: `changes` over what the server offers.
//!
//! - **sync-collection** (RFC 6578) when the server serves it: the anchor is `sync:<token>`, the
//!   report is asked at `sync-level` `infinite`, and a removed folder is taken apart into its
//!   files from the listing this replica holds.
//! - **an etag tree walk** when it does not (Nextcloud's files): PROPFIND depth 1 down the folder,
//!   diffed against the last walk. The anchor is `tree:<replica>:<n>`, good only for the replica
//!   that handed it out, so after a restart it is `AnchorExpired` and the listing is reconciled
//!   by the engine (nothing known is fetched or uploaded again).
//! - a long answer is handed out in pages: the anchor `more:<replica>:<batch>:<offset>` reads the
//!   next page of a batch held in memory, and ends in the anchor the whole batch came to.

use crate::entry::{Entry, Kind, entries};
use crate::refuse::read_error;
use crate::replica::{DELETED, WebDavReplica};
use crate::requests;
use porter_core::Bytes;
use porter_dav::{Multistatus, parse_multistatus, parse_sync_collection, token_expired};
use porter_http::{Http, HttpResponse};
use porter_sync::{
    Anchor, Change, ChangePage, Cursor, More, RemoteId, RemoteVersion, ReplicaError, RetryAfter,
    Tombstone,
};
use std::collections::{BTreeMap, BTreeSet};

/// Which feed this server gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// sync-collection.
    Sync,
    /// A walk of the tree.
    Tree,
}

/// One file as last listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub version: RemoteVersion,
    pub size: Bytes,
}

/// The files as last listed, and for a tree walk the anchor that listing was handed out as.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Known {
    pub files: BTreeMap<RemoteId, Meta>,
    pub tag: Option<Anchor>,
}

/// A batch of changes held back for the pages after the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub batch: u64,
    pub changes: Vec<Change>,
    pub end: Anchor,
}

/// Everything one read of the server came to.
struct Batch {
    changes: Vec<Change>,
    end: Anchor,
}

/// Why a sync-collection report gave no changes.
enum Report {
    /// The server does not serve this report (or not at this level): use the tree.
    Unsupported,
    /// The token is no longer good.
    Expired,
    /// The read failed in a way the caller reports.
    Failed(ReplicaError),
}

/// What a report listed.
struct Reported {
    token: String,
    files: Vec<Entry>,
    removed: Vec<RemoteId>,
}

/// An anchor read back.
enum Parsed<'a> {
    Sync(&'a str),
    Tree,
    More { batch: u64, offset: usize },
    Unknown,
}

fn parse<'a>(anchor: &'a Anchor, nonce: &str) -> Parsed<'a> {
    let text = anchor.0.as_str();
    if let Some(token) = text.strip_prefix("sync:") {
        return Parsed::Sync(token);
    }
    let own = |rest: &str| {
        rest.strip_prefix(nonce)
            .and_then(|r| r.strip_prefix(':'))
            .map(str::to_owned)
    };
    if let Some(rest) = text.strip_prefix("tree:") {
        return own(rest).map_or(Parsed::Unknown, |_| Parsed::Tree);
    }
    let more = text.strip_prefix("more:").and_then(own).and_then(|rest| {
        let (batch, offset) = rest.split_once(':')?;
        Some((batch.parse().ok()?, offset.parse().ok()?))
    });
    match more {
        Some((batch, offset)) => Parsed::More { batch, offset },
        None => Parsed::Unknown,
    }
}

impl<H: Http> WebDavReplica<H> {
    pub(crate) async fn feed(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
        let batch = match from {
            Cursor::Start => self.listing().await?,
            Cursor::At(anchor) => match parse(&anchor, &self.nonce) {
                Parsed::Sync(token) => self.since(token).await?,
                Parsed::Tree => self.walked_since(&anchor).await?,
                Parsed::More { batch, offset } => return self.page_of(batch, offset),
                Parsed::Unknown => return Err(ReplicaError::AnchorExpired),
            },
        };
        Ok(self.paged(batch))
    }

    /// The first page of `batch`, the rest held for `page_of`.
    fn paged(&self, batch: Batch) -> ChangePage {
        let Batch { changes, end } = batch;
        if changes.len() <= self.page {
            return ChangePage {
                changes,
                next: end,
                more: More::Done,
            };
        }
        let mut state = self.state();
        state.counter += 1;
        let id = state.counter;
        let first = changes[..self.page].to_vec();
        state.pending = Some(Pending {
            batch: id,
            changes,
            end,
        });
        ChangePage {
            changes: first,
            next: self.more_anchor(id, self.page),
            more: More::More,
        }
    }

    /// The page of the held batch that starts at `offset`; a batch this replica no longer holds
    /// is an expired anchor.
    fn page_of(&self, batch: u64, offset: usize) -> Result<ChangePage, ReplicaError> {
        let state = self.state();
        let pending = state
            .pending
            .as_ref()
            .filter(|p| p.batch == batch && offset <= p.changes.len())
            .ok_or(ReplicaError::AnchorExpired)?;
        let stop = offset.saturating_add(self.page).min(pending.changes.len());
        let (next, more) = match stop == pending.changes.len() {
            true => (pending.end.clone(), More::Done),
            false => (self.more_anchor(batch, stop), More::More),
        };
        Ok(ChangePage {
            changes: pending.changes[offset..stop].to_vec(),
            next,
            more,
        })
    }

    fn more_anchor(&self, batch: u64, offset: usize) -> Anchor {
        Anchor(format!("more:{}:{batch}:{offset}", self.nonce))
    }

    /// Every file now, from a sync-collection report with no token if the server serves one,
    /// else from a walk.
    async fn listing(&self) -> Result<Batch, ReplicaError> {
        let known_mode = self.state().mode;
        if known_mode != Some(Mode::Tree) {
            match self.report("").await {
                Ok(reported) => return Ok(self.first_listing(reported)),
                Err(Report::Failed(error)) => return Err(error),
                Err(Report::Unsupported | Report::Expired) => {}
            }
        }
        let files = self.walk().await?;
        Ok(self.first_walk(files))
    }

    fn first_listing(&self, reported: Reported) -> Batch {
        let mut state = self.state();
        state.mode = Some(Mode::Sync);
        let items: Vec<Entry> = by_id(reported.files);
        state.known = Some(known_of(&items, None));
        Batch {
            changes: upserts(&self.root, &items),
            end: Anchor(format!("sync:{}", reported.token)),
        }
    }

    fn first_walk(&self, files: Vec<Entry>) -> Batch {
        let mut state = self.state();
        state.mode = Some(Mode::Tree);
        state.counter += 1;
        let end = Anchor(format!("tree:{}:{}", self.nonce, state.counter));
        state.known = Some(known_of(&files, Some(end.clone())));
        Batch {
            changes: upserts(&self.root, &files),
            end,
        }
    }

    /// Changes after a sync-collection `token`.
    async fn since(&self, token: &str) -> Result<Batch, ReplicaError> {
        let reported = match self.report(token).await {
            Ok(reported) => reported,
            Err(Report::Expired) => return Err(ReplicaError::AnchorExpired),
            Err(Report::Unsupported) => return Err(ReplicaError::AnchorExpired),
            Err(Report::Failed(error)) => return Err(error),
        };
        let mut state = self.state();
        state.mode = Some(Mode::Sync);
        let files = by_id(reported.files);
        let mut changes = upserts(&self.root, &files);
        match state.known.as_mut() {
            Some(known) => {
                for entry in &files {
                    known.files.insert(entry.id.clone(), meta_of(entry));
                }
                for gone in &reported.removed {
                    changes.extend(self.take_down(known, gone));
                }
            }
            // Nothing is held to say which files a removed folder held: list again.
            None if !reported.removed.is_empty() => return Err(ReplicaError::AnchorExpired),
            None => {}
        }
        Ok(Batch {
            changes,
            end: Anchor(format!("sync:{}", reported.token)),
        })
    }

    /// The tombstones for `gone` and everything below it, which `known` forgets.
    fn take_down(&self, known: &mut Known, gone: &RemoteId) -> Vec<Change> {
        let below = format!("{}/", gone.0);
        let ids: Vec<RemoteId> = known
            .files
            .keys()
            .filter(|id| *id == gone || id.0.starts_with(&below))
            .cloned()
            .collect();
        ids.into_iter()
            .map(|id| {
                known.files.remove(&id);
                Change::Tombstone(Tombstone {
                    id,
                    version: RemoteVersion(DELETED.to_owned()),
                    deleted_at: self.clock.now(),
                })
            })
            .collect()
    }

    /// Changes since the walk that was handed out as `anchor`.
    async fn walked_since(&self, anchor: &Anchor) -> Result<Batch, ReplicaError> {
        let holds = self
            .state()
            .known
            .as_ref()
            .is_some_and(|known| known.tag.as_ref() == Some(anchor));
        if !holds {
            return Err(ReplicaError::AnchorExpired);
        }
        let files = self.walk().await?;
        let mut state = self.state();
        let Some(known) = state
            .known
            .as_ref()
            .filter(|k| k.tag.as_ref() == Some(anchor))
        else {
            return Err(ReplicaError::AnchorExpired);
        };
        let changes = diff(&self.root, known, &files, self.clock.now());
        if changes.is_empty() {
            return Ok(Batch {
                changes,
                end: anchor.clone(),
            });
        }
        state.counter += 1;
        let end = Anchor(format!("tree:{}:{}", self.nonce, state.counter));
        state.known = Some(known_of(&files, Some(end.clone())));
        Ok(Batch { changes, end })
    }

    /// A sync-collection report from `token`.
    async fn report(&self, token: &str) -> Result<Reported, Report> {
        let url = self
            .root
            .folder_url()
            .ok_or(Report::Failed(ReplicaError::Gone))?;
        let response = self
            .send(requests::sync(url, token))
            .await
            .map_err(Report::Failed)?;
        let body = String::from_utf8_lossy(&response.body).into_owned();
        match response.status.0 {
            207 => self.reported(&body),
            code if token_expired(code, &body) => Err(Report::Expired),
            401 | 429 | 500 | 502..=599 => Err(Report::Failed(read_error(&response))),
            _ => Err(Report::Unsupported),
        }
    }

    fn reported(&self, body: &str) -> Result<Reported, Report> {
        let status: Multistatus = parse_multistatus(body).map_err(|_| Report::Unsupported)?;
        let reply = parse_sync_collection(&status).map_err(|_| Report::Unsupported)?;
        // A reply with no token is the server cutting the report short, or not serving it.
        let token = reply.sync_token.ok_or(Report::Expired)?;
        let removed = status
            .responses
            .iter()
            .filter(|r| r.status == Some(404))
            .filter_map(|r| self.root.id_of(&r.href))
            .collect();
        let files = entries(&self.root, &status)
            .into_iter()
            .filter(|e| e.kind == Kind::File)
            .collect();
        Ok(Reported {
            token,
            files,
            removed,
        })
    }

    /// Every file under the folder, by PROPFIND depth 1 down the tree.
    async fn walk(&self) -> Result<Vec<Entry>, ReplicaError> {
        let mut folders = vec![self.root.folder()];
        let mut seen: BTreeSet<RemoteId> = BTreeSet::new();
        let mut files = Vec::new();
        while let Some(folder) = folders.pop() {
            if !seen.insert(folder.clone()) {
                continue;
            }
            let url = self
                .root
                .collection_url(&folder)
                .ok_or(ReplicaError::Gone)?;
            let response = self.send(requests::list(url)).await?;
            let listed = self.listed(&response, &folder)?;
            for entry in listed {
                match entry.kind {
                    Kind::Folder if entry.id != folder => folders.push(entry.id),
                    Kind::Folder => {}
                    Kind::File => files.push(entry),
                }
            }
        }
        Ok(by_id(files))
    }

    /// What a depth-1 listing of `folder` held. A folder that vanished since its parent was
    /// listed has nothing; the dataset's own folder gone is `Gone`.
    fn listed(
        &self,
        response: &HttpResponse,
        folder: &RemoteId,
    ) -> Result<Vec<Entry>, ReplicaError> {
        match response.status.0 {
            207 => {
                let text = String::from_utf8_lossy(&response.body);
                let status = parse_multistatus(&text)
                    .map_err(|_| ReplicaError::Transient(RetryAfter(30)))?;
                Ok(entries(&self.root, &status))
            }
            404 if *folder != self.root.folder() => Ok(Vec::new()),
            _ => Err(read_error(response)),
        }
    }
}

fn meta_of(entry: &Entry) -> Meta {
    Meta {
        version: entry.version.clone(),
        size: entry.size,
    }
}

fn known_of(files: &[Entry], tag: Option<Anchor>) -> Known {
    Known {
        files: files.iter().map(|e| (e.id.clone(), meta_of(e))).collect(),
        tag,
    }
}

/// `files` once each, ordered by id.
fn by_id(files: Vec<Entry>) -> Vec<Entry> {
    let map: BTreeMap<RemoteId, Entry> = files.into_iter().map(|e| (e.id.clone(), e)).collect();
    map.into_values().collect()
}

fn upserts(root: &crate::path::Root, files: &[Entry]) -> Vec<Change> {
    files
        .iter()
        .filter_map(|entry| entry.item(root))
        .map(Change::Upsert)
        .collect()
}

/// What changed from `known` to `now`: an upsert for each file that is new or has another etag,
/// a tombstone for each that is gone.
fn diff(
    root: &crate::path::Root,
    known: &Known,
    now: &[Entry],
    at: porter_core::UnixSeconds,
) -> Vec<Change> {
    let changed: Vec<Entry> = now
        .iter()
        .filter(|e| {
            known
                .files
                .get(&e.id)
                .is_none_or(|old| old.version != e.version)
        })
        .cloned()
        .collect();
    let present: BTreeSet<&RemoteId> = now.iter().map(|e| &e.id).collect();
    let mut changes = upserts(root, &changed);
    changes.extend(
        known
            .files
            .keys()
            .filter(|id| !present.contains(id))
            .map(|id| {
                Change::Tombstone(Tombstone {
                    id: id.clone(),
                    version: RemoteVersion(DELETED.to_owned()),
                    deleted_at: at,
                })
            }),
    );
    changes
}
