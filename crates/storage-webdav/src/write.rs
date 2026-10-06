//! Writes: `put` and `remove`, each guarded so a stale base is a `Conflict` and never an
//! overwrite. A new item is a PUT with `If-None-Match: *`, a changed one a PUT with `If-Match`
//! on the version the writer last saw, a removal a DELETE with `If-Match`. A `412` is read back
//! by asking the server what is there now.

use crate::entry::{Kind, entries};
use crate::path::parents_of;
use crate::refuse::{write_error, write_unreached};
use crate::replica::{DELETED, WebDavReplica};
use crate::requests;
use porter_dav::parse_multistatus;
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use porter_sync::{
    BaseVersion, Conflict, PutItem, PutRefused, PutTarget, RemoteId, RemoteSide, RemoteVersion,
    RetryAfter,
};

impl<H: Http> WebDavReplica<H> {
    pub(crate) async fn write(
        &self,
        item: PutItem,
        base: BaseVersion,
    ) -> Result<(RemoteId, RemoteVersion), PutRefused> {
        let (id, creating) = match &item.target {
            PutTarget::New(path) => (self.root.id_for(path), true),
            PutTarget::Existing(id) => (self.root.item_path(id).map(|_| id.clone()), false),
        };
        let id = id.ok_or(PutRefused::Forbidden)?;
        let guard = match (&base, creating) {
            (_, true) => Some(("If-None-Match", "*".to_owned())),
            (BaseVersion::At(seen), false) => Some(("If-Match", seen.0.clone())),
            // An existing item written on the belief that it is absent: it is not.
            (BaseVersion::Absent, false) => return Err(self.conflict(&id, &base).await),
        };
        let url = self.root.url_of(&id).ok_or(PutRefused::Forbidden)?;
        let request = guard.into_iter().fold(
            HttpRequest::new(Method::Put, url)
                .with_header("Content-Type", "application/octet-stream")
                .with_body(item.content.0),
            |request, (name, value)| request.with_header(name, value),
        );
        let mut response = self.write_send(request.clone()).await?;
        if response.status.0 == 409 && creating {
            // A folder on the way is missing: make them, then once more.
            self.make_parents(&id).await?;
            response = self.write_send(request).await?;
        }
        match response.status.0 {
            200 | 201 | 204 => Ok((id.clone(), self.version_after(&id, &response).await)),
            404 | 412 => Err(self.conflict(&id, &base).await),
            _ => Err(write_error(&response)),
        }
    }

    pub(crate) async fn delete(
        &self,
        item: &RemoteId,
        base: BaseVersion,
    ) -> Result<RemoteVersion, PutRefused> {
        let BaseVersion::At(seen) = &base else {
            return Err(self.conflict(item, &base).await);
        };
        let url = self.root.url_of(item).ok_or(PutRefused::Forbidden)?;
        let request = HttpRequest::new(Method::Delete, url).with_header("If-Match", seen.0.clone());
        let response = self.write_send(request).await?;
        match response.status.0 {
            200 | 204 => Ok(RemoteVersion(DELETED.to_owned())),
            404 | 412 => Err(self.conflict(item, &base).await),
            _ => Err(write_error(&response)),
        }
    }

    async fn write_send(&self, request: HttpRequest) -> Result<HttpResponse, PutRefused> {
        self.http.send(request).await.map_err(write_unreached)
    }

    /// Makes each folder between the dataset's and `id`. A folder that exists (`405`) is fine.
    async fn make_parents(&self, id: &RemoteId) -> Result<(), PutRefused> {
        for folder in parents_of(&self.root, id) {
            if self.state().folders.contains(&folder) {
                continue;
            }
            let url = self
                .root
                .collection_url(&folder)
                .ok_or(PutRefused::Forbidden)?;
            let response = self
                .write_send(HttpRequest::new(Method::Mkcol, url))
                .await?;
            match response.status.0 {
                200 | 201 | 405 => {
                    self.state().folders.insert(folder);
                }
                _ => return Err(write_error(&response)),
            }
        }
        Ok(())
    }

    /// The version a successful PUT made: the `ETag` it answered with, else what the server
    /// says the item is now, else nothing the next read cannot settle.
    async fn version_after(&self, id: &RemoteId, response: &HttpResponse) -> RemoteVersion {
        if let Some(tag) = response.header("etag") {
            return RemoteVersion(tag.to_owned());
        }
        match self.current(id).await {
            Ok(Some(version)) => version,
            _ => RemoteVersion(String::new()),
        }
    }

    /// The version of the file at `id` now, or `None` when there is none.
    async fn current(&self, id: &RemoteId) -> Result<Option<RemoteVersion>, PutRefused> {
        let url = self.root.url_of(id).ok_or(PutRefused::Forbidden)?;
        let response = self.write_send(requests::stat(url)).await?;
        match response.status.0 {
            207 => {
                let text = String::from_utf8_lossy(&response.body);
                let status =
                    parse_multistatus(&text).map_err(|_| PutRefused::Transient(RetryAfter(30)))?;
                Ok(entries(&self.root, &status)
                    .into_iter()
                    .find(|e| e.id == *id && e.kind == Kind::File)
                    .map(|e| e.version))
            }
            404 => Ok(None),
            _ => Err(write_error(&response)),
        }
    }

    /// The conflict a refused write is: what the server has at `id` now.
    async fn conflict(&self, id: &RemoteId, base: &BaseVersion) -> PutRefused {
        let remote = match (self.current(id).await, base) {
            (Err(refused), _) => return refused,
            (Ok(Some(version)), BaseVersion::Absent) => RemoteSide::Exists(id.clone(), version),
            (Ok(Some(version)), BaseVersion::At(_)) => RemoteSide::Changed(version),
            (Ok(None), BaseVersion::At(_)) => {
                RemoteSide::Deleted(RemoteVersion(DELETED.to_owned()))
            }
            // The item was there a moment ago and is gone now: the write can be tried again.
            (Ok(None), BaseVersion::Absent) => return PutRefused::Transient(RetryAfter(1)),
        };
        PutRefused::Conflict(Conflict {
            item: id.clone(),
            base: base.clone(),
            remote,
        })
    }
}
