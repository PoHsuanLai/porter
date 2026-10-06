//! [`WebDavReplica`]: porter-sync's [`Replica`] over one folder of a WebDAV server.

use crate::clock::Clock;
use crate::feed::{Known, Pending};
use crate::path::Root;
use crate::refuse::{read_error, unreached};
use crate::requests;
use porter_core::capability::{
    Access, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_core::{Bytes, WebUrl};
use porter_dav::parse_multistatus;
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use porter_sync::{
    BaseVersion, Blob, ByteRange, ChangePage, Cursor, PutItem, PutRefused, Quota, RemoteId,
    RemoteVersion, Replica, ReplicaError,
};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// How many changes a page of the feed holds unless told otherwise.
const PAGE: usize = 500;

/// The version a deletion has: a WebDAV server keeps none for what it removed, so the replica
/// names it, and a removal, the feed's tombstone and a refused write all agree.
pub const DELETED: &str = "deleted";

/// What the replica remembers between calls; losing it (a new process) only expires anchors.
#[derive(Debug, Default)]
pub(crate) struct State {
    /// What kind of feed the server gave, once known.
    pub mode: Option<crate::feed::Mode>,
    /// The files as last listed, so a removed folder can name its files and a tree walk can diff.
    pub known: Option<Known>,
    /// A listing or feed too big for one page, handed out page by page.
    pub pending: Option<Pending>,
    /// How many batches and tree walks this replica has made, for anchors.
    pub counter: u64,
    /// Folders known to exist, so a PUT does not make them again.
    pub folders: std::collections::BTreeSet<RemoteId>,
}

/// The replica of one WebDAV folder, over any [`Http`]: in syncd a [`crate::StreamHttp`] on
/// the relay `OpenAuthenticated` opens, in a test the fake server.
#[derive(Debug)]
pub struct WebDavReplica<H> {
    pub(crate) http: H,
    pub(crate) root: Root,
    pub(crate) clock: Clock,
    pub(crate) nonce: String,
    pub(crate) page: usize,
    pub(crate) state: Mutex<State>,
}

impl<H: Http> WebDavReplica<H> {
    /// The replica of the folder at `folder`, sending through `http` and dating tombstones
    /// with `clock`.
    pub fn new(http: H, folder: &WebUrl, clock: Clock) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        Self {
            http,
            root: Root::new(folder),
            clock,
            nonce: format!("{nonce:x}"),
            page: PAGE,
            state: Mutex::default(),
        }
    }

    /// The same replica with feed pages of `size` changes (at least one).
    pub fn with_page(self, size: usize) -> Self {
        Self {
            page: size.max(1),
            ..self
        }
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        // A panic with the lock held already failed the test that caused it.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends a request, a transport failure being a read that should be retried.
    pub(crate) async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ReplicaError> {
        self.http.send(request).await.map_err(unreached)
    }

    /// A GET of `id`, with a `Range` header when `range` says so.
    async fn get(&self, id: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        let url = self.root.url_of(id).ok_or(ReplicaError::Gone)?;
        let mut request = HttpRequest::new(Method::Get, url);
        if let ByteRange::Span {
            start: Bytes(start),
            len: Bytes(len),
        } = range
        {
            if len == 0 {
                return Ok(Blob(Vec::new()));
            }
            let last = start.saturating_add(len - 1);
            request = request.with_header("Range", format!("bytes={start}-{last}"));
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
            _ => Err(read_error(&response)),
        }
    }

    async fn quota_of_folder(&self) -> Result<Quota, ReplicaError> {
        let url = self.root.folder_url().ok_or(ReplicaError::Gone)?;
        let response = self.send(requests::quota(url)).await?;
        if response.status.0 != 207 {
            return Err(read_error(&response));
        }
        let text = String::from_utf8_lossy(&response.body);
        let status = parse_multistatus(&text)
            .map_err(|_| ReplicaError::Transient(porter_sync::RetryAfter(30)))?;
        let reported = porter_dav::quota(&status);
        let used = reported.used.unwrap_or(0);
        Ok(Quota {
            used: Bytes(used),
            total: reported
                .available
                .map(|room| Bytes(used.saturating_add(room))),
        })
    }
}

impl<H: Http> Replica for WebDavReplica<H> {
    async fn changes(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
        self.feed(from).await
    }

    async fn fetch(&self, item: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        self.get(item, range).await
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
        self.quota_of_folder().await
    }

    fn features(&self) -> StorageCap {
        StorageCap {
            access: Access::ReadWrite,
            // WebDAV has no push; a change is found by asking.
            delta: Delta::Poll,
            quota: QuotaReport::Reported,
            scope: StorageScope::Full,
            // Nextcloud's checksums are SHA-1 or MD5 and only SHA-256 is one this build compares,
            // so identity is by etag, and by the bytes where an etag differs.
            hashes: HashKind::None,
            ranges: Offered::Present,
            // One PUT per item until the chunked upload (Nextcloud's `uploads` area) is built.
            chunked_upload: Offered::Absent,
        }
    }
}
