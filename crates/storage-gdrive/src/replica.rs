//! [`GdriveReplica`]: porter-sync's [`Replica`] over a folder of Drive's app data folder.

use crate::addr::{Addr, FILE_FIELDS};
use crate::json::{About, File};
use crate::refuse::{read_error, unreached};
use porter_core::capability::{
    Access, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_core::{Bytes, WebUrl};
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use porter_sync::{
    BaseVersion, Blob, ByteRange, ChangePage, Cursor, PutItem, PutRefused, Quota, RemoteId,
    RemoteVersion, Replica, ReplicaError, RetryAfter,
};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use storage_webdav::Clock;

/// How many items a page asks for unless told otherwise.
const PAGE: usize = 200;

/// The largest file sent as one multipart request. The same threshold as the Graph replica's
/// (Drive recommends multipart up to 5 MB).
pub const SIMPLE_MAX: usize = 4_000_000;

/// What a non-final chunk of a resumable upload must be a multiple of (256 KiB).
pub const CHUNK_UNIT: usize = 262_144;

/// The chunk a resumable upload sends unless told otherwise: 40 units (10 MiB).
const CHUNK: usize = 40 * CHUNK_UNIT;

/// When a write is one multipart request and when it is a resumable session, and the chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uploads {
    /// The largest file sent as one request.
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
/// correctness.
#[derive(Debug, Default)]
pub(crate) struct State {
    /// The dataset folder's id.
    pub root: Option<String>,
    /// Folders seen: id to (name, parent id).
    pub folders: HashMap<String, (String, Option<String>)>,
    /// Files seen: id to parent id, so a deleted folder can name its files.
    pub files: HashMap<String, String>,
}

/// The replica of one folder of the app data folder, over any [`Http`].
#[derive(Debug)]
pub struct GdriveReplica<H> {
    pub(crate) http: H,
    pub(crate) addr: Addr,
    pub(crate) clock: Clock,
    pub(crate) page: usize,
    pub(crate) uploads: Uploads,
    pub(crate) state: Mutex<State>,
}

impl<H: Http> GdriveReplica<H> {
    /// The replica of `folder` (a path below the app data folder, empty for the app data folder
    /// itself) of the API served at `base` (`https://www.googleapis.com/drive/v3`), sending
    /// through `http` and dating tombstones with `clock`.
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

    /// The same replica asking for pages of `size` items (at least one).
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

    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        // A panic with the lock held already failed the test that caused it.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends a request, a transport failure being a read that should be retried.
    pub(crate) async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ReplicaError> {
        self.http
            .send(request.with_header("Accept", "application/json"))
            .await
            .map_err(unreached)
    }

    /// A GET of `url`.
    pub(crate) async fn get_url(&self, url: Option<WebUrl>) -> Result<HttpResponse, ReplicaError> {
        let url = url.ok_or(ReplicaError::Gone)?;
        self.send(HttpRequest::new(Method::Get, url)).await
    }

    /// The file `id` now (also in the trash), or `None` when there is none.
    pub(crate) async fn file_now(&self, id: &str) -> Result<Option<File>, ReplicaError> {
        let url = self.addr.file(id, &[("fields", FILE_FIELDS)]);
        let response = self.get_url(url).await?;
        match response.status.0 {
            200 => serde_json::from_slice(&response.body)
                .map(Some)
                .map_err(|_| ReplicaError::Transient(RetryAfter(30))),
            404 => Ok(None),
            _ => Err(read_error(&response)),
        }
    }

    /// A GET of `id`'s content, with a `Range` header when `range` says so.
    async fn content(&self, id: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        let mut request = HttpRequest::new(
            Method::Get,
            self.addr
                .file(&id.0, &[("alt", "media")])
                .ok_or(ReplicaError::Gone)?,
        );
        if let ByteRange::Span {
            start: Bytes(start),
            len: Bytes(len),
        } = range
        {
            if len == 0 {
                return Ok(Blob(Vec::new()));
            }
            request = request.with_header(
                "Range",
                format!("bytes={start}-{}", start.saturating_add(len - 1)),
            );
        }
        let response = self.send(request).await?;
        match (response.status.0, range) {
            (206, _) | (200, ByteRange::Whole) => Ok(Blob(response.body)),
            (200, ByteRange::Span { start, len }) => {
                // The server ignored the range: the span is cut out here.
                let from = usize::try_from(start.0)
                    .unwrap_or(usize::MAX)
                    .min(response.body.len());
                let to = from
                    .saturating_add(usize::try_from(len.0).unwrap_or(usize::MAX))
                    .min(response.body.len());
                Ok(Blob(response.body[from..to].to_vec()))
            }
            (416, _) => Ok(Blob(Vec::new())),
            // A file in the trash, or gone, reads as gone.
            _ => Err(read_error(&response)),
        }
    }

    async fn quota_of_about(&self) -> Result<Quota, ReplicaError> {
        let url = self.addr.api("about", &[("fields", "storageQuota")]);
        let response = self.get_url(url).await?;
        if response.status.0 != 200 {
            return Err(read_error(&response));
        }
        let about: About = serde_json::from_slice(&response.body)
            .map_err(|_| ReplicaError::Transient(RetryAfter(30)))?;
        let number = |text: Option<String>| text.and_then(|t| t.parse::<u64>().ok());
        let quota = about.quota;
        Ok(Quota {
            used: Bytes(number(quota.as_ref().and_then(|q| q.usage.clone())).unwrap_or(0)),
            // An account with no limit sends none (or zero).
            total: number(quota.and_then(|q| q.limit))
                .filter(|total| *total > 0)
                .map(Bytes),
        })
    }
}

impl<H: Http> Replica for GdriveReplica<H> {
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
        self.quota_of_about().await
    }

    fn features(&self) -> StorageCap {
        StorageCap {
            access: Access::ReadWrite,
            // Drive can push through watch channels, which need a public endpoint; a change is
            // found by asking.
            delta: Delta::Poll,
            quota: QuotaReport::Reported,
            // The replica touches the app data folder only, whatever the token's scope allows.
            scope: StorageScope::AppFolder,
            // Drive reports an MD5 for each binary file; this build compares SHA-256 only, so
            // an item whose version differs is compared by its bytes.
            hashes: HashKind::Md5,
            ranges: Offered::Present,
            chunked_upload: Offered::Present,
        }
    }
}
