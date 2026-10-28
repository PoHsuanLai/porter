//! Whole-drive writes ([`DriveWrite`]) in My Drive: folders and files made in a parent by id,
//! new content for a file by id, renames and moves as `files.update` (`addParents`,
//! `removeParents`), and the trash as `files.update` with `trashed: true`; never `files.delete`,
//! which deletes for good. Drive documents no conditional update, so each change of an existing
//! item reads its `version` first and refuses a stale base as a `Conflict` (the window is one
//! round trip, as for `put`).
//!
//! A file up to [`crate::SIMPLE_MAX`] is one request; a larger one goes in a resumable session,
//! read from its file a chunk at a time. The session's URL is handed to the caller to keep, and
//! an upload given one asks it what it has (`Content-Range: bytes */<size>`, answered `308` with
//! a `Range`) and sends the rest; a session Drive has dropped is begun again.
//!
//! Drive refuses almost no name: any but an empty one is taken. It takes a second item of a name,
//! so a create whose answer was lost and is sent again would make a twin: each new item is made
//! under an id reserved for it first (`files.generateIds`, space `drive`; Drive takes them for
//! files and folders, not for its own documents), and a create of an id already used is refused
//! (`409`, per Google's "Create and populate files" guide), which finds the item made.
//!
//! Drive has no `If-Match` either: a resumable session that replaces a file asks again what the
//! file is before its last chunk and when it is resumed.

use crate::addr::FILE_FIELDS;
use crate::feed::link;
use crate::json::{File, GeneratedIds};
use crate::refuse::write_error;
use crate::replica::GdriveReplica;
use crate::write::Target;
use porter_http::http::header::CONTENT_TYPE;
use porter_http::http::{Method, Response};
use porter_http::{HeaderText, Http, request};
use porter_ids::WebUrl;
use porter_ids::percent::encode_unreserved;
use porter_sync::status::{refused, write_unreached};
use porter_sync::{
    BaseVersion, Conflict, DriveWrite, Found, Place, PutRefused, RemoteId, RemoteSide,
    RemoteVersion, RetryAfter, Session, Upload, UploadTo, Written,
};

fn again() -> PutRefused {
    PutRefused::Transient(RetryAfter(30))
}

/// Whether Drive takes `name`: any but an empty one.
pub fn accepts_name(name: &str) -> bool {
    !name.is_empty()
}

impl<H: Http> GdriveReplica<H> {
    /// The id of `place`'s folder: its own, or My Drive's (the dataset folder's) for the top.
    async fn parent_id(&self, place: &Place) -> Result<String, PutRefused> {
        match &place.parent {
            Some(id) => Ok(id.0.clone()),
            None => self.root_id(false).await.map_err(refused),
        }
    }

    /// The file `id` now, if it is still at `base`; else the conflict that is.
    async fn at_base(&self, id: &RemoteId, base: &BaseVersion) -> Result<File, PutRefused> {
        let target = Target::Existing(id.clone());
        let now = self.file_now(&id.0).await.map_err(refused)?;
        match (&now, base) {
            (Some(file), BaseVersion::At(seen))
                if !file.in_trash() && file.version.as_deref() == Some(seen.0.as_str()) =>
            {
                Ok(file.clone())
            }
            _ => Err(Self::conflict(&target, base, now.as_ref())),
        }
    }

    async fn settled(
        &self,
        response: Response<Vec<u8>>,
        target: &Target,
        base: &BaseVersion,
    ) -> Result<Written, PutRefused> {
        self.settle(response, target, base)
            .await
            .map(|(id, version)| Written { id, version })
    }

    /// The metadata of a new item at `place`, under the id reserved for it.
    fn new_meta(place: &Place, parent: &str) -> serde_json::Value {
        let mut meta = serde_json::json!({"name": place.name, "parents": [parent]});
        if let Some(id) = &place.id {
            meta["id"] = serde_json::Value::String(id.0.clone());
        }
        meta
    }

    /// A create of `place` was refused `409`: its reserved id is used, by the item an earlier
    /// try made. That item, as a conflict for the caller to take.
    async fn made_before(&self, place: &Place) -> PutRefused {
        let Some(id) = &place.id else {
            return PutRefused::Declined;
        };
        match self.file_now(&id.0).await {
            Err(error) => refused(error),
            Ok(Some(file)) if !file.in_trash() => PutRefused::Conflict(Conflict {
                item: id.clone(),
                base: BaseVersion::Absent,
                remote: RemoteSide::Exists(
                    id.clone(),
                    RemoteVersion(file.version.unwrap_or_default()),
                ),
            }),
            // Used, and in the trash or gone: a new id is wanted.
            Ok(_) => PutRefused::Declined,
        }
    }

    async fn drive_make_folder(&self, place: &Place) -> Result<Written, PutRefused> {
        let parent = self.parent_id(place).await?;
        let mut meta = Self::new_meta(place, &parent);
        meta["mimeType"] = serde_json::Value::String(crate::json::FOLDER.to_owned());
        let body = meta.to_string();
        let url = self
            .addr
            .api("files", &[("fields", FILE_FIELDS)])
            .ok_or(PutRefused::Forbidden)?;
        let make = request(Method::POST, &url)
            .header(CONTENT_TYPE, "application/json")
            .body(body.into_bytes())
            .map_err(|e| write_unreached(e.into()))?;
        let response = self.write_send(make).await?;
        if response.status().as_u16() == 409 {
            return Err(self.made_before(place).await);
        }
        let target = Target::New {
            parent,
            name: place.name.clone(),
        };
        self.settled(response, &target, &BaseVersion::Absent).await
    }

    async fn drive_upload(&self, upload: Upload<'_>) -> Result<Written, PutRefused> {
        let size = upload.content.size();
        let small = size <= self.uploads.simple_max as u64;
        let (target, meta) = match &upload.to {
            UploadTo::New(place) => {
                let parent = self.parent_id(place).await?;
                let meta = Self::new_meta(place, &parent);
                (
                    Target::New {
                        parent,
                        name: place.name.clone(),
                    },
                    meta.to_string(),
                )
            }
            UploadTo::Replace(id) => {
                self.at_base(id, &upload.base).await?;
                (Target::Existing(id.clone()), "{}".to_owned())
            }
        };
        let response = match (small, &upload.to) {
            (true, to) => {
                let bytes = upload.content.read(0, size).map_err(|_| again())?;
                let response = match to {
                    UploadTo::New(_) => self.create_small(&meta, bytes).await?,
                    UploadTo::Replace(id) => self.update_small(&id.0, bytes).await?,
                };
                upload.progress.sent(size);
                response
            }
            (false, _) => self.resumable(&upload, &meta).await?,
        };
        if let (409, UploadTo::New(place)) = (response.status().as_u16(), &upload.to) {
            return Err(self.made_before(place).await);
        }
        self.settled(response, &target, &upload.base).await
    }

    /// Whether the file an upload replaces is still the version it was made against; else the
    /// conflict that is.
    async fn still_at_base(&self, upload: &Upload<'_>) -> Result<(), PutRefused> {
        match &upload.to {
            UploadTo::Replace(id) => self.at_base(id, &upload.base).await.map(drop),
            UploadTo::New(_) => Ok(()),
        }
    }

    /// Where a session from before stands: the first byte it still wants, the finished file's
    /// answer, or `None` when Drive no longer has it.
    async fn session_at(&self, session: &Session, size: u64) -> Result<Resumed, PutRefused> {
        let ours = |url: &WebUrl| Some(url.origin()) == self.addr.origin();
        let Some(url) = link(session.address()).filter(ours) else {
            return Ok(Resumed::Gone);
        };
        let ask = request(Method::PUT, &url)
            .header("Content-Range", format!("bytes */{size}"))
            .body(Vec::new())
            .map_err(|e| write_unreached(e.into()))?;
        let response = self.write_send(ask).await?;
        match response.status().as_u16() {
            308 => {
                let from = response
                    .header("range")
                    .and_then(|r| r.rsplit('-').next())
                    .and_then(|last| last.parse::<u64>().ok())
                    .map_or(0, |last| last + 1);
                Ok(Resumed::At(url, from))
            }
            200 | 201 => Ok(Resumed::Done(response)),
            404 | 410 => Ok(Resumed::Gone),
            _ => Err(write_error(&response, self.clock.now())),
        }
    }

    /// Opens a resumable session for `upload` described by `meta`: its URL, or the refusal as
    /// the response.
    async fn open_session(
        &self,
        upload: &Upload<'_>,
        meta: &str,
    ) -> Result<Result<WebUrl, Response<Vec<u8>>>, PutRefused> {
        let pairs = [("uploadType", "resumable"), ("fields", FILE_FIELDS)];
        let start = match &upload.to {
            UploadTo::New(_) => self.addr.upload("files", &pairs),
            UploadTo::Replace(id) => self
                .addr
                .upload(&format!("files/{}", encode_unreserved(&id.0)), &pairs),
        };
        let mut open = request(Method::POST, &start.ok_or(PutRefused::Forbidden)?)
            .header(CONTENT_TYPE, "application/json; charset=UTF-8")
            .header("X-Upload-Content-Type", "application/octet-stream")
            .header("X-Upload-Content-Length", upload.content.size().to_string());
        if matches!(upload.to, UploadTo::Replace(_)) {
            open = open.header("X-HTTP-Method-Override", "PATCH");
        }
        let open = open
            .body(meta.as_bytes().to_vec())
            .map_err(|e| write_unreached(e.into()))?;
        let created = self.write_send(open).await?;
        if created.status().as_u16() != 200 {
            return Ok(Err(created));
        }
        let url = created
            .header("location")
            .and_then(link)
            .filter(|url| Some(url.origin()) == self.addr.origin())
            .ok_or_else(again)?;
        upload
            .progress
            .session(Some(&Session::new(url.as_str().to_owned())));
        Ok(Ok(url))
    }

    /// Sends `upload` in a resumable session, resuming the one it names when Drive still has it.
    async fn resumable(
        &self,
        upload: &Upload<'_>,
        meta: &str,
    ) -> Result<Response<Vec<u8>>, PutRefused> {
        let total = upload.content.size();
        let resumed = match &upload.resume {
            Some(session) => self.session_at(session, total).await?,
            None => Resumed::Gone,
        };
        let (url, mut sent) = match resumed {
            Resumed::Done(response) => {
                upload.progress.sent(total);
                upload.progress.session(None);
                return Ok(response);
            }
            Resumed::At(url, from) => {
                // A session from before: the file it replaces may have changed since.
                if let Err(why) = self.still_at_base(upload).await {
                    upload.progress.session(None);
                    return Err(why);
                }
                (url, from)
            }
            Resumed::Gone => {
                if upload.resume.is_some() {
                    upload.progress.session(None);
                }
                match self.open_session(upload, meta).await? {
                    Ok(url) => (url, 0),
                    Err(refusal) => return Ok(refusal),
                }
            }
        };
        upload.progress.sent(sent);
        while sent < total {
            if upload.progress.held() {
                // Paused: the session stays, to be resumed.
                return Err(PutRefused::Transient(RetryAfter(0)));
            }
            let chunk = upload
                .content
                .read(sent, self.uploads.chunk as u64)
                .map_err(|_| again())?;
            let last = sent + chunk.len() as u64 - 1;
            // The last chunk commits the file: what it replaces is asked about first.
            if last + 1 == total
                && let Err(why) = self.still_at_base(upload).await
            {
                upload.progress.session(None);
                return Err(why);
            }
            let piece = request(Method::PUT, &url)
                .header(CONTENT_TYPE, "application/octet-stream")
                .header("Content-Range", format!("bytes {sent}-{last}/{total}"))
                .body(chunk)
                .map_err(|e| write_unreached(e.into()))?;
            // Not reached: the session stays, to be resumed.
            let response = self.write_send(piece).await?;
            match response.status().as_u16() {
                308 => {
                    sent = last + 1;
                    upload.progress.sent(sent);
                }
                200 | 201 => {
                    upload.progress.sent(total);
                    upload.progress.session(None);
                    return Ok(response);
                }
                // Throttled, or Drive failed: the session stays, to be resumed.
                429 | 500..=599 => return Ok(response),
                _ => {
                    upload.progress.session(None);
                    return Ok(response);
                }
            }
        }
        Err(again())
    }

    /// A `files.update` of `id`'s metadata, with these parent changes.
    async fn update_meta(
        &self,
        id: &RemoteId,
        meta: serde_json::Value,
        parents: &[(&str, &str)],
    ) -> Result<Response<Vec<u8>>, PutRefused> {
        let mut pairs = vec![("fields", FILE_FIELDS)];
        pairs.extend_from_slice(parents);
        let url = self.addr.file(&id.0, &pairs).ok_or(PutRefused::Forbidden)?;
        let update = request(Method::POST, &url)
            .header("X-HTTP-Method-Override", "PATCH")
            .header(CONTENT_TYPE, "application/json")
            .body(meta.to_string().into_bytes())
            .map_err(|e| write_unreached(e.into()))?;
        self.write_send(update).await
    }

    async fn drive_relocate(
        &self,
        item: &RemoteId,
        base: &BaseVersion,
        to: &Place,
    ) -> Result<Written, PutRefused> {
        let now = self.at_base(item, base).await?;
        let parent = self.parent_id(to).await?;
        let old = now.parent_id().map(str::to_owned);
        let moves = old.as_deref() != Some(parent.as_str());
        let mut parents = Vec::new();
        if moves {
            parents.push(("addParents", parent.as_str()));
            if let Some(old) = old.as_deref() {
                parents.push(("removeParents", old));
            }
        }
        let response = self
            .update_meta(item, serde_json::json!({"name": to.name}), &parents)
            .await?;
        self.settled(response, &Target::Existing(item.clone()), base)
            .await
    }

    async fn drive_trash(&self, item: &RemoteId, base: &BaseVersion) -> Result<(), PutRefused> {
        self.at_base(item, base).await?;
        let response = self
            .update_meta(item, serde_json::json!({"trashed": true}), &[])
            .await?;
        match response.status().as_u16() {
            200 => {
                self.state().folders.remove(item);
                Ok(())
            }
            _ => self
                .settled(response, &Target::Existing(item.clone()), base)
                .await
                .map(drop),
        }
    }

    async fn drive_look(&self, item: &RemoteId) -> Result<Option<Found>, PutRefused> {
        let now = self.file_now(&item.0).await.map_err(refused)?;
        Ok(now.filter(|file| !file.in_trash()).map(|file| Found {
            version: RemoteVersion(file.version.clone().unwrap_or_default()),
            size: file.bytes(),
            hash: file.hash(),
            folder: file.is_folder(),
        }))
    }

    async fn drive_reserve_ids(&self, count: usize) -> Result<Vec<RemoteId>, PutRefused> {
        let count = count.clamp(1, 1000).to_string();
        let url = self
            .addr
            .api(
                "files/generateIds",
                &[("count", &count), ("space", "drive")],
            )
            .ok_or(PutRefused::Forbidden)?;
        let get = request(Method::GET, &url)
            .body(Vec::new())
            .map_err(|e| write_unreached(e.into()))?;
        let response = self.write_send(get).await?;
        match response.status().as_u16() {
            200 => {
                let ids: GeneratedIds =
                    serde_json::from_slice(response.body()).map_err(|_| again())?;
                Ok(ids.ids.into_iter().map(RemoteId).collect())
            }
            _ => Err(write_error(&response, self.clock.now())),
        }
    }
}

/// Where a resumable session from before stands.
enum Resumed {
    /// Drive wants the bytes from here on.
    At(WebUrl, u64),
    /// It had them all: the finished file's answer.
    Done(Response<Vec<u8>>),
    /// Drive no longer has it.
    Gone,
}

impl<H: Http> DriveWrite for GdriveReplica<H> {
    fn accepts_name(&self, name: &str) -> bool {
        accepts_name(name)
    }

    async fn look(&self, item: &RemoteId) -> Result<Option<Found>, PutRefused> {
        self.drive_look(item).await
    }

    async fn reserve_ids(&self, count: usize) -> Result<Vec<RemoteId>, PutRefused> {
        self.drive_reserve_ids(count).await
    }

    async fn make_folder(&self, place: &Place) -> Result<Written, PutRefused> {
        self.drive_make_folder(place).await
    }

    async fn upload(&self, upload: Upload<'_>) -> Result<Written, PutRefused> {
        self.drive_upload(upload).await
    }

    async fn relocate(
        &self,
        item: &RemoteId,
        base: &BaseVersion,
        to: &Place,
    ) -> Result<Written, PutRefused> {
        self.drive_relocate(item, base, to).await
    }

    async fn trash(&self, item: &RemoteId, base: &BaseVersion) -> Result<(), PutRefused> {
        self.drive_trash(item, base).await
    }
}
