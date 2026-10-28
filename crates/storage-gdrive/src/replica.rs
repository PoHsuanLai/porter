//! [`GdriveReplica`]: porter-sync's [`Replica`] over a folder of Drive's app data folder, or of
//! My Drive.

use crate::addr::{Addr, FILE_FIELDS, Root};
use crate::json::{About, File};
use crate::refuse::{export_error, read_error};
use porter_capability::{Access, Delta, HashKind, StorageCap, StorageScope};
use porter_http::http::header::{ACCEPT, HeaderValue, RANGE};
use porter_http::http::{Method, Request, Response};
use porter_http::{Http, request};
use porter_ids::{Bytes, WebUrl};
use porter_sync::Clock;
use porter_sync::status::unreached;
use porter_sync::{
    BaseVersion, Blob, ByteRange, ChangePage, Cursor, ExportError, Folders, PutItem, PutRefused,
    Quota, RemoteId, RemoteVersion, Replica, ReplicaError, RetryAfter,
};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// How many items a page asks for unless told otherwise.
const PAGE: usize = 200;

/// The largest file sent as one multipart request. The same threshold as the Graph replica's
/// (Drive recommends multipart up to 5 MB).
pub const SIMPLE_MAX: usize = 4_000_000;

/// What a non-final chunk of a resumable upload must be a multiple of (256 KiB).
pub const CHUNK_UNIT: usize = 262_144;

/// The chunk a resumable upload sends unless told otherwise: 40 units (10 MiB).
const CHUNK: usize = 40 * CHUNK_UNIT;

/// The most `files.export` gives: Drive exports a document of up to 10 MB.
pub const EXPORT_LIMIT: u64 = 10_000_000;

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
/// correctness, except that a folder deleted since is not known to have held its files unless
/// the tree was kept ([`GdriveReplica::folders`]) and handed back.
#[derive(Debug, Default)]
pub(crate) struct State {
    /// The dataset folder's id.
    pub root: Option<String>,
    /// Folders seen, and the parent of each file seen, so a deleted folder can name its files.
    pub folders: Folders,
}

/// The replica of one folder of the app data folder or of My Drive, over any [`Http`].
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
    /// The replica of `folder` (a path below the root, empty for the root itself: the app data
    /// folder unless [`GdriveReplica::with_root`] says otherwise) of the API served at `base` (`https://www.googleapis.com/drive/v3`), sending
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

    /// The same replica with its folder below `root` ([`Root::AppFolder`] unless told otherwise).
    pub fn with_root(self, root: Root) -> Self {
        Self {
            addr: self.addr.with_root(root),
            ..self
        }
    }

    /// The same replica starting from a tree an earlier one learnt ([`GdriveReplica::folders`]).
    /// A listing from the start forgets it, as it forgets everything else seen.
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

    /// The file `id` now (also in the trash), or `None` when there is none.
    pub(crate) async fn file_now(&self, id: &str) -> Result<Option<File>, ReplicaError> {
        let url = self.addr.file(id, &[("fields", FILE_FIELDS)]);
        let response = self.get_url(url).await?;
        match response.status().as_u16() {
            200 => serde_json::from_slice(response.body())
                .map(Some)
                .map_err(|_| ReplicaError::Transient(RetryAfter(30))),
            404 => Ok(None),
            _ => Err(read_error(&response, self.clock.now())),
        }
    }

    /// A GET of `id`'s content, with a `Range` header when `range` says so.
    async fn content(&self, id: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        let url = self
            .addr
            .file(&id.0, &[("alt", "media")])
            .ok_or(ReplicaError::Gone)?;
        let mut get = request(Method::GET, &url);
        if let ByteRange::Span {
            start: Bytes(start),
            len: Bytes(len),
        } = range
        {
            if len == 0 {
                return Ok(Blob(Vec::new()));
            }
            get = get.header(
                RANGE,
                format!("bytes={start}-{}", start.saturating_add(len - 1)),
            );
        }
        let get = get.body(Vec::new()).map_err(|e| unreached(e.into()))?;
        let response = self.send(get).await?;
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
            // A file in the trash, or gone, reads as gone.
            _ => Err(read_error(&response, self.clock.now())),
        }
    }

    /// `files.export` of the document `id` as `mime`: Drive exports up to [`EXPORT_LIMIT`].
    async fn exported(&self, id: &RemoteId, mime: &str) -> Result<Blob, ExportError> {
        let url = self
            .addr
            .api(
                &format!(
                    "files/{}/export",
                    porter_ids::percent::encode_unreserved(&id.0)
                ),
                &[("mimeType", mime)],
            )
            .ok_or(ExportError::Read(ReplicaError::Gone))?;
        // No `Accept: application/json`: the answer is the file.
        let get = request(Method::GET, &url)
            .body(Vec::new())
            .map_err(|e| ExportError::Read(unreached(e.into())))?;
        let response = self
            .http
            .send(get)
            .await
            .map_err(|e| ExportError::Read(unreached(e)))?;
        match response.status().as_u16() {
            200 if response.body().len() as u64 > EXPORT_LIMIT => Err(ExportError::TooLarge),
            200 => Ok(Blob(response.into_body())),
            _ => Err(export_error(&response, self.clock.now())),
        }
    }

    async fn quota_of_about(&self) -> Result<Quota, ReplicaError> {
        let url = self.addr.api("about", &[("fields", "storageQuota")]);
        let response = self.get_url(url).await?;
        if response.status().as_u16() != 200 {
            return Err(read_error(&response, self.clock.now()));
        }
        let about: About = serde_json::from_slice(response.body())
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

    async fn export(&self, item: &RemoteId, mime: &str) -> Result<Blob, ExportError> {
        self.exported(item, mime).await
    }

    fn features(&self) -> StorageCap {
        StorageCap {
            access: Access::ReadWrite,
            // Drive can push through watch channels, which need a public endpoint; a change is
            // found by asking.
            delta: Delta::Poll,
            quota: true,
            // The replica touches its root only, whatever the token's scope allows.
            scope: match self.addr.root() {
                Root::AppFolder => StorageScope::AppFolder,
                Root::Drive => StorageScope::Full,
            },
            // Drive reports an MD5 for each binary file; this build compares SHA-256 only, so
            // an item whose version differs is compared by its bytes.
            hashes: HashKind::Md5,
            ranges: true,
            chunked_upload: true,
        }
    }
}
