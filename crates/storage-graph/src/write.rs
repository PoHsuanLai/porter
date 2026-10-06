//! Writes: `put` and `remove`, each guarded so a stale base is a `Conflict` and never an
//! overwrite. A new item is created with `@microsoft.graph.conflictBehavior=fail` (`409` if the
//! name is taken), a changed one is written with `If-Match` on the eTag the writer last saw
//! (`412` if it moved), a removal is a DELETE with `If-Match`. A refusal is read back by asking
//! the server what is there now.

use crate::addr::names_of;
use crate::json::DriveItem;
use crate::refuse::{refused, write_error, write_unreached};
use crate::replica::GraphReplica;
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use porter_sync::{
    BaseVersion, Conflict, PutItem, PutRefused, PutTarget, RemoteId, RemoteSide, RemoteVersion,
    RetryAfter,
};
use storage_webdav::DELETED;

/// Where a write goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// A new item at these names below the dataset's folder.
    New(Vec<String>),
    /// An item that exists.
    Existing(RemoteId),
}

type Written = Result<(RemoteId, RemoteVersion), PutRefused>;

impl<H: Http> GraphReplica<H> {
    pub(crate) async fn write(&self, item: PutItem, base: BaseVersion) -> Written {
        let target = match &item.target {
            PutTarget::New(path) => Target::New(names_of(&path.0).ok_or(PutRefused::Forbidden)?),
            PutTarget::Existing(id) => Target::Existing(id.clone()),
        };
        let guard = match (&target, &base) {
            (Target::New(_), _) => None,
            (Target::Existing(_), BaseVersion::At(seen)) => Some(seen.0.clone()),
            // An existing item written on the belief that it is absent: it is not.
            (Target::Existing(_), BaseVersion::Absent) => {
                return Err(self.conflict(&target, &base, false).await);
            }
        };
        let bytes = item.content.0;
        let response = match bytes.len() <= self.uploads.simple_max || bytes.is_empty() {
            true => self.put_simple(&target, guard.as_deref(), bytes).await?,
            false => self.put_session(&target, guard.as_deref(), bytes).await?,
        };
        self.settle(response, &target, &base).await
    }

    pub(crate) async fn delete(
        &self,
        item: &RemoteId,
        base: BaseVersion,
    ) -> Result<RemoteVersion, PutRefused> {
        let target = Target::Existing(item.clone());
        let BaseVersion::At(seen) = &base else {
            return Err(self.conflict(&target, &base, false).await);
        };
        let url = self
            .addr
            .item(&item.0, None, "")
            .ok_or(PutRefused::Forbidden)?;
        let request = HttpRequest::new(Method::Delete, url).with_header("If-Match", seen.0.clone());
        let response = self.write_send(request).await?;
        match response.status.0 {
            200 | 204 => {
                self.state().files.remove(&item.0);
                Ok(RemoteVersion(DELETED.to_owned()))
            }
            404 | 412 => Err(self.conflict(&target, &base, false).await),
            _ => Err(write_error(&response)),
        }
    }

    pub(crate) async fn write_send(
        &self,
        request: HttpRequest,
    ) -> Result<HttpResponse, PutRefused> {
        self.http
            .send(request.with_header("Accept", "application/json"))
            .await
            .map_err(write_unreached)
    }

    /// One PUT of the whole content.
    async fn put_simple(
        &self,
        target: &Target,
        guard: Option<&str>,
        bytes: Vec<u8>,
    ) -> Result<HttpResponse, PutRefused> {
        let url = match target {
            Target::New(names) => self.addr.path(
                Some(names),
                Some("content"),
                "?@microsoft.graph.conflictBehavior=fail",
            ),
            Target::Existing(id) => self.addr.item(&id.0, Some("content"), ""),
        };
        let request = HttpRequest::new(Method::Put, url.ok_or(PutRefused::Forbidden)?)
            .with_header("Content-Type", "application/octet-stream")
            .with_body(bytes);
        let request = match guard {
            Some(tag) => request.with_header("If-Match", tag),
            None => request,
        };
        self.write_send(request).await
    }

    /// What a finished write's answer comes to.
    pub(crate) async fn settle(
        &self,
        response: HttpResponse,
        target: &Target,
        base: &BaseVersion,
    ) -> Written {
        match response.status.0 {
            200 | 201 => {
                let item: DriveItem = serde_json::from_slice(&response.body)
                    .map_err(|_| PutRefused::Transient(RetryAfter(30)))?;
                if let Some(parent) = item.parent_id() {
                    self.state()
                        .files
                        .insert(item.id.clone(), parent.to_owned());
                }
                Ok((
                    RemoteId(item.id),
                    RemoteVersion(item.etag.unwrap_or_default()),
                ))
            }
            404 | 412 => Err(self.conflict(target, base, false).await),
            409 => Err(self.conflict(target, base, true).await),
            _ => Err(write_error(&response)),
        }
    }

    /// The conflict a refused write is: what the server has there now. `blocked` says the server
    /// answered `409`, so an item that is not there means the path itself is in the way.
    async fn conflict(&self, target: &Target, base: &BaseVersion, blocked: bool) -> PutRefused {
        let now = match target {
            Target::Existing(id) => self.item_now(&id.0).await,
            Target::New(names) => self.read_item(self.addr.path(Some(names), None, "")).await,
        };
        let remote = match (now, target, base) {
            (Err(error), ..) => return refused(error),
            (Ok(Some(item)), _, BaseVersion::Absent) => RemoteSide::Exists(
                RemoteId(item.id),
                RemoteVersion(item.etag.unwrap_or_default()),
            ),
            (Ok(Some(item)), _, BaseVersion::At(_)) => {
                RemoteSide::Changed(RemoteVersion(item.etag.unwrap_or_default()))
            }
            (Ok(None), Target::Existing(_), BaseVersion::At(_)) => {
                RemoteSide::Deleted(RemoteVersion(DELETED.to_owned()))
            }
            // Nothing there, though the server said something was: a name on the path is a file
            // (`409`), or the item was there a moment ago and the write can be tried again.
            (Ok(None), ..) if blocked => return PutRefused::Forbidden,
            (Ok(None), ..) => return PutRefused::Transient(RetryAfter(1)),
        };
        let item = match (target, &remote) {
            (Target::Existing(id), _) => id.clone(),
            (Target::New(_), RemoteSide::Exists(id, _)) => id.clone(),
            (Target::New(_), _) => RemoteId(String::new()),
        };
        PutRefused::Conflict(Conflict {
            item,
            base: base.clone(),
            remote,
        })
    }
}
