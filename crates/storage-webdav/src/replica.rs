//! [`WebDavReplica`]: porter-sync's [`Replica`] over one folder of a WebDAV server.

use crate::feed::{Known, Pending};
use crate::path::Root;
use crate::requests;
use dav_proto::parse_multistatus;
use porter_capability::{Access, Delta, HashKind, StorageCap, StorageScope};
use porter_http::http::header::RANGE;
use porter_http::http::{Method, Request, Response};
use porter_http::{Http, request};
use porter_ids::{Bytes, WebUrl};
use porter_sync::Clock;
use porter_sync::status::{read_error, unreached};
use porter_sync::{
    BaseVersion, Blob, ByteRange, ChangePage, Cursor, PutItem, PutRefused, Quota, RemoteId,
    RemoteVersion, Replica, ReplicaError,
};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// How many changes a page of the feed holds unless told otherwise.
const PAGE: usize = 500;

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

/// The replica of one WebDAV folder, over any [`Http`]: in syncd a `porter_http_relay::RelayHttp` on
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
    /// with `clock`. `nonce` is what tells this run's anchors from an earlier run's, so a
    /// cursor the host kept across a restart expires instead of being read against state it no
    /// longer has: it must differ between two runs of the host, and may be the same for the
    /// replicas of one run (letters and digits only: it is a field of the anchors, and a colon ends it).
    pub fn new(http: H, folder: &WebUrl, clock: Clock, nonce: &str) -> Self {
        Self {
            http,
            root: Root::new(folder),
            clock,
            nonce: nonce.to_owned(),
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
    pub(crate) async fn send(
        &self,
        request: Request<Vec<u8>>,
    ) -> Result<Response<Vec<u8>>, ReplicaError> {
        self.http.send(request).await.map_err(unreached)
    }

    /// A GET of `id`, with a `Range` header when `range` says so.
    async fn get(&self, id: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        let url = self.root.url_of(id).ok_or(ReplicaError::Gone)?;
        let mut get = request(Method::GET, &url);
        if let ByteRange::Span {
            start: Bytes(start),
            len: Bytes(len),
        } = range
        {
            if len == 0 {
                return Ok(Blob(Vec::new()));
            }
            let last = start.saturating_add(len - 1);
            get = get.header(RANGE, format!("bytes={start}-{last}"));
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
            _ => Err(read_error(&response, self.clock.now())),
        }
    }

    async fn quota_of_folder(&self) -> Result<Quota, ReplicaError> {
        let url = self.root.folder_url().ok_or(ReplicaError::Gone)?;
        let response = self.send(requests::quota(url).map_err(unreached)?).await?;
        if response.status().as_u16() != 207 {
            return Err(read_error(&response, self.clock.now()));
        }
        let text = String::from_utf8_lossy(response.body());
        let status = parse_multistatus(&text)
            .map_err(|_| ReplicaError::Transient(porter_sync::RetryAfter(30)))?;
        let reported = dav_proto::quota(&status);
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
            quota: true,
            scope: StorageScope::Full,
            // Nextcloud's checksums are SHA-1 or MD5 and only SHA-256 is one this build compares,
            // so identity is by etag, and by the bytes where an etag differs.
            hashes: HashKind::None,
            ranges: true,
            // One PUT per item until the chunked upload (Nextcloud's `uploads` area) is built.
            chunked_upload: false,
        }
    }
}
