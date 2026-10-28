//! [`GraphReplica`]: porter-sync's [`Replica`] over a folder of OneDrive's app folder, or of the
//! whole drive.

use crate::addr::{Addr, Root};
use crate::json::{DriveInfo, DriveItem};
use porter_capability::{Access, Delta, HashKind, StorageCap, StorageScope};
use porter_http::http::header::{ACCEPT, HeaderValue, RANGE};
use porter_http::http::{Method, Request, Response};
use porter_http::{HeaderText, Http, request};
use porter_ids::{Bytes, WebUrl};
use porter_sync::Clock;
use porter_sync::status::{read_error, unreached};
use porter_sync::{
    BaseVersion, Blob, ByteRange, ChangePage, Cursor, Folders, PutItem, PutRefused, Quota,
    RemoteId, RemoteVersion, Replica, ReplicaError, RetryAfter,
};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// How many delta items a page asks for unless told otherwise.
const PAGE: usize = 200;

/// The largest file sent as one PUT: Graph's simple upload takes up to 4 MB.
pub const SIMPLE_MAX: usize = 4_000_000;

/// What a non-final chunk of an upload session must be a multiple of (320 KiB).
pub const CHUNK_UNIT: usize = 327_680;

/// The chunk an upload session sends unless told otherwise: 32 units (10 MiB), well under
/// Graph's 60 MiB limit.
const CHUNK: usize = 32 * CHUNK_UNIT;

/// When a write is one PUT and when it is a session, and the session's chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uploads {
    /// The largest file sent as one PUT.
    pub simple_max: usize,
    /// The chunk of a session: a multiple of [`CHUNK_UNIT`], at least one.
    pub chunk: usize,
}

impl Uploads {
    /// These limits, the chunk rounded down to a whole number of units (at least one).
    pub fn new(simple_max: usize, chunk: usize) -> Self {
        Self {
            simple_max,
            chunk: (chunk / CHUNK_UNIT).max(1) * CHUNK_UNIT,
        }
    }
}

impl Default for Uploads {
    fn default() -> Self {
        Self::new(SIMPLE_MAX, CHUNK)
    }
}

/// What the replica remembers between calls; losing it (a new process) costs requests, never
/// correctness, except that a folder deleted since is not known to have held its files unless
/// the tree was kept ([`GraphReplica::folders`]) and handed back.
#[derive(Debug, Default)]
pub(crate) struct State {
    /// The dataset folder's item id.
    pub root: Option<String>,
    /// Folders seen, and the parent of each file seen, so a deleted folder can name its files.
    pub folders: Folders,
}

/// The replica of one folder of the app folder or of the whole drive, over any [`Http`].
#[derive(Debug)]
pub struct GraphReplica<H> {
    pub(crate) http: H,
    pub(crate) addr: Addr,
    pub(crate) clock: Clock,
    pub(crate) page: usize,
    pub(crate) uploads: Uploads,
    pub(crate) state: Mutex<State>,
}

impl<H: Http> GraphReplica<H> {
    /// The replica of `folder` (a path below the root, empty for the root itself: the app folder
    /// unless [`GraphReplica::with_root`] says otherwise) of the drive served at `base` (`https://graph.microsoft.com`), sending through `http` and
    /// dating tombstones with `clock`.
    pub fn new(http: H, base: &WebUrl, folder: &str, clock: Clock) -> Self {
        Self {
            http,
            addr: Addr::new(base, folder),
            clock,
            page: PAGE,
            uploads: Uploads::default(),
            state: Mutex::default(),
        }
    }

    /// The same replica asking for delta pages of `size` items (at least one).
    pub fn with_page(self, size: usize) -> Self {
        Self {
            page: size.max(1),
            ..self
        }
    }

    /// The same replica with these upload limits.
    pub fn with_uploads(self, uploads: Uploads) -> Self {
        Self { uploads, ..self }
    }

    /// The same replica with its folder below `root` ([`Root::AppFolder`] unless told otherwise).
    pub fn with_root(self, root: Root) -> Self {
        Self {
            addr: self.addr.with_root(root),
            ..self
        }
    }

    /// The same replica starting from a tree an earlier one learnt ([`GraphReplica::folders`]).
    pub fn with_folders(self, folders: Folders) -> Self {
        let state = State {
            root: None,
            folders,
        };
        Self {
            state: Mutex::new(state),
            ..self
        }
    }

    /// The tree learnt so far, for a dataset to keep beside the anchor of the last page it
    /// applied and hand to the next replica.
    pub fn folders(&self) -> Folders {
        self.state().folders.clone()
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        // A panic with the lock held already failed the test that caused it.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends a request, a transport failure being a read that should be retried.
    pub(crate) async fn send(
        &self,
        mut request: Request<Vec<u8>>,
    ) -> Result<Response<Vec<u8>>, ReplicaError> {
        request
            .headers_mut()
            .append(ACCEPT, HeaderValue::from_static("application/json"));
        self.http.send(request).await.map_err(unreached)
    }

    /// A GET of `url`.
    pub(crate) async fn get_url(
        &self,
        url: Option<WebUrl>,
    ) -> Result<Response<Vec<u8>>, ReplicaError> {
        let url = url.ok_or(ReplicaError::Gone)?;
        let get = request(Method::GET, &url)
            .body(Vec::new())
            .map_err(|e| unreached(e.into()))?;
        self.send(get).await
    }

    /// The item `id` now, or `None` when there is none.
    pub(crate) async fn item_now(&self, id: &str) -> Result<Option<DriveItem>, ReplicaError> {
        self.read_item(self.addr.item(id, None, "")).await
    }

    pub(crate) async fn read_item(
        &self,
        url: Option<WebUrl>,
    ) -> Result<Option<DriveItem>, ReplicaError> {
        let response = self.get_url(url).await?;
        match response.status().as_u16() {
            200 => serde_json::from_slice(response.body())
                .map(Some)
                .map_err(|_| ReplicaError::Transient(RetryAfter(30))),
            404 => Ok(None),
            _ => Err(read_error(&response, self.clock.now())),
        }
    }

    /// A GET of `id`'s content, with a `Range` header when `range` says so. Graph answers a
    /// download with a redirect to a pre-authenticated URL: it is followed once.
    async fn content(&self, id: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        let mut headers = Vec::new();
        if let ByteRange::Span {
            start: Bytes(start),
            len: Bytes(len),
        } = range
        {
            if len == 0 {
                return Ok(Blob(Vec::new()));
            }
            headers.push(format!("bytes={start}-{}", start.saturating_add(len - 1)));
        }
        let ranged = |url: WebUrl| {
            headers
                .iter()
                .fold(request(Method::GET, &url), |r, range| {
                    r.header(RANGE, range.as_str())
                })
                .body(Vec::new())
                .map_err(|e| unreached(e.into()))
        };
        let url = self.addr.item(&id.0, Some("content"), "");
        let mut response = self.send(ranged(url.ok_or(ReplicaError::Gone)?)?).await?;
        if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            let target = response
                .header("location")
                .and_then(|l| WebUrl::parse(l).ok())
                .ok_or(ReplicaError::Transient(RetryAfter(30)))?;
            response = self.send(ranged(target)?).await?;
        }
        match (response.status().as_u16(), range) {
            (206, _) | (200, ByteRange::Whole) => Ok(Blob(response.into_body())),
            (200, ByteRange::Span { start, len }) => {
                // The server ignored the range: the span is cut out here.
                let body = response.body();
                let from = usize::try_from(start.0)
                    .unwrap_or(usize::MAX)
                    .min(body.len());
                let to = from
                    .saturating_add(usize::try_from(len.0).unwrap_or(usize::MAX))
                    .min(body.len());
                Ok(Blob(body[from..to].to_vec()))
            }
            (416, _) => Ok(Blob(Vec::new())),
            _ => Err(read_error(&response, self.clock.now())),
        }
    }

    async fn quota_of_drive(&self) -> Result<Quota, ReplicaError> {
        let response = self.get_url(self.addr.drive()).await?;
        if response.status().as_u16() != 200 {
            return Err(read_error(&response, self.clock.now()));
        }
        let info: DriveInfo = serde_json::from_slice(response.body())
            .map_err(|_| ReplicaError::Transient(RetryAfter(30)))?;
        let quota = info.quota;
        Ok(Quota {
            used: Bytes(quota.as_ref().and_then(|q| q.used).unwrap_or(0)),
            // A drive that reports no limit says `total: 0` or none.
            total: quota
                .and_then(|q| q.total)
                .filter(|total| *total > 0)
                .map(Bytes),
        })
    }
}

impl<H: Http> Replica for GraphReplica<H> {
    async fn changes(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
        self.feed(from).await
    }

    async fn fetch(&self, item: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        self.content(item, range).await
    }

    async fn put(
        &self,
        item: PutItem,
        base: BaseVersion,
    ) -> Result<(RemoteId, RemoteVersion), PutRefused> {
        self.write(item, base).await
    }

    async fn remove(
        &self,
        item: &RemoteId,
        base: BaseVersion,
    ) -> Result<RemoteVersion, PutRefused> {
        self.delete(item, base).await
    }

    async fn quota(&self) -> Result<Quota, ReplicaError> {
        self.quota_of_drive().await
    }

    fn features(&self) -> StorageCap {
        StorageCap {
            access: Access::ReadWrite,
            // No push for a Graph drive here (change notifications need a public endpoint);
            // a change is found by asking.
            delta: Delta::Poll,
            quota: true,
            // The replica touches its root only, whatever the token's scope allows.
            scope: match self.addr.root() {
                Root::Drive => StorageScope::Full,
                Root::AppFolder => StorageScope::AppFolder,
            },
            // OneDrive reports QuickXorHash; this build compares SHA-256 only, so an item whose
            // eTag differs is compared by its bytes.
            hashes: HashKind::QuickXor,
            ranges: true,
            chunked_upload: true,
        }
    }
}
