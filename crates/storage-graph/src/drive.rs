//! Whole-drive writes ([`DriveWrite`]): folders made with `conflictBehavior=fail`, uploads into a
//! parent by id (`items/{parent}:/{name}:/content`) or over an item by id, renames and moves as
//! a `PATCH`, and the recycle bin as a `DELETE` (OneDrive keeps a deleted item there). Every
//! change of an existing item carries `If-Match` with the eTag it was made against, so a stale
//! one is a `Conflict` read back from the server.
//!
//! A file up to [`crate::SIMPLE_MAX`] is one PUT; a larger one goes in an upload session, read
//! from its file a chunk at a time. The session's URL is handed to the caller to keep, and an
//! upload given one asks it what it has (`GET`: `nextExpectedRanges`) and sends the rest; a
//! session the server has dropped is begun again.
//!
//! The names OneDrive refuses ([`accepts_name`]) are those of Microsoft's "Restrictions and
//! limitations in OneDrive and SharePoint" (support.microsoft.com, article
//! 64883a5d-228e-48f5-b3d2-eb39e07630fa, read 2026-10): the characters `" * : < > ? / \ |`, a
//! leading or trailing space, the names `.lock`, `CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`,
//! `LPT0`-`LPT9`, `desktop.ini`, a name starting with `~$`, and `_vti_` anywhere. A trailing dot
//! (which Windows drops) and a name over 255 characters are refused here too.

use crate::feed::{folder_of, json_string};
use crate::json::{DriveItem, Session as SessionJson, SessionStatus};
use crate::refuse::{refused, write_error};
use crate::replica::GraphReplica;
use porter_core::WebUrl;
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use porter_sync::{
    BaseVersion, Conflict, DriveWrite, Place, PutRefused, RemoteId, RemoteSide, RemoteVersion,
    RetryAfter, Session, Upload, UploadTo, Written,
};
use storage_webdav::DELETED;

/// The characters OneDrive refuses in a name.
const REFUSED_CHARS: &[char] = &['"', '*', ':', '<', '>', '?', '/', '\\', '|'];

/// The names OneDrive refuses, in any case.
const REFUSED_NAMES: &[&str] = &[".lock", "con", "prn", "aux", "nul", "desktop.ini"];

/// The longest name taken.
const NAME_MAX: usize = 255;

/// Whether OneDrive takes `name` for a file or folder (see the module's list).
pub fn accepts_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let device = |prefix: &str| {
        lower
            .strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
    };
    !name.is_empty()
        && name.chars().count() <= NAME_MAX
        && !name.contains(REFUSED_CHARS)
        && !name.starts_with(' ')
        && !name.ends_with(' ')
        && !name.ends_with('.')
        && !name.starts_with("~$")
        && !lower.contains("_vti_")
        && !REFUSED_NAMES.contains(&lower.as_str())
        && !device("com")
        && !device("lpt")
}

fn again() -> PutRefused {
    PutRefused::Transient(RetryAfter(30))
}

/// The eTag a base names, or the conflict an existing item written on the belief that it is
/// absent is.
fn guard_of(base: &BaseVersion) -> Option<&str> {
    match base {
        BaseVersion::At(seen) => Some(seen.0.as_str()),
        BaseVersion::Absent => None,
    }
}

impl<H: Http> GraphReplica<H> {
    /// The id of `place`'s folder: its own, or the dataset folder's for the top.
    async fn parent_id(&self, place: &Place) -> Result<String, PutRefused> {
        match &place.parent {
            Some(id) => Ok(id.0.clone()),
            None => self.root_id(false).await.map_err(refused),
        }
    }

    /// What a write that made or changed an item answered with: the item.
    fn written(&self, response: &HttpResponse) -> Result<(Written, DriveItem), PutRefused> {
        let item: DriveItem = serde_json::from_slice(&response.body).map_err(|_| again())?;
        let written = Written {
            id: RemoteId(item.id.clone()),
            version: RemoteVersion(item.etag.clone().unwrap_or_default()),
        };
        Ok((written, item))
    }

    /// Notes a written item in the tree, so the feed finds its path.
    fn remember(&self, item: &DriveItem) {
        let id = RemoteId(item.id.clone());
        let mut state = self.state();
        match (item.folder.is_some(), item.parent_id()) {
            (true, _) => state.folders.insert_folder(id, folder_of(item)),
            (false, Some(parent)) => state.folders.insert_file(id, RemoteId(parent.to_owned())),
            (false, None) => {}
        }
    }

    /// The conflict a refused change of `item` is: what the server has now.
    async fn changed_conflict(&self, item: &RemoteId, base: &BaseVersion) -> PutRefused {
        let remote = match self.item_now(&item.0).await {
            Err(error) => return refused(error),
            Ok(Some(now)) => RemoteSide::Changed(RemoteVersion(now.etag.unwrap_or_default())),
            Ok(None) => RemoteSide::Deleted(RemoteVersion(DELETED.to_owned())),
        };
        PutRefused::Conflict(Conflict {
            item: item.clone(),
            base: base.clone(),
            remote,
        })
    }

    /// The conflict a new item whose name is taken is: the item there.
    async fn taken_conflict(&self, place: &Place) -> PutRefused {
        let parent = match self.parent_id(place).await {
            Ok(parent) => parent,
            Err(why) => return why,
        };
        let url = self.addr.child(Some(&parent), &place.name, None, "");
        match self.read_item(url).await {
            Err(error) => refused(error),
            Ok(Some(item)) => PutRefused::Conflict(Conflict {
                item: RemoteId(item.id.clone()),
                base: BaseVersion::Absent,
                remote: RemoteSide::Exists(
                    RemoteId(item.id),
                    RemoteVersion(item.etag.unwrap_or_default()),
                ),
            }),
            // Taken a moment ago and gone now: try again.
            Ok(None) => PutRefused::Transient(RetryAfter(1)),
        }
    }

    /// What a finished write of `to` comes to.
    async fn settle_upload(
        &self,
        response: HttpResponse,
        to: &UploadTo,
        base: &BaseVersion,
    ) -> Result<Written, PutRefused> {
        match (response.status.0, to) {
            (200 | 201, _) => {
                let (written, item) = self.written(&response)?;
                self.remember(&item);
                Ok(written)
            }
            (404 | 412, UploadTo::Replace(id)) => Err(self.changed_conflict(id, base).await),
            (409, UploadTo::New(place)) => Err(self.taken_conflict(place).await),
            _ => Err(write_error(&response)),
        }
    }

    async fn drive_upload(&self, upload: Upload<'_>) -> Result<Written, PutRefused> {
        if let (UploadTo::Replace(id), BaseVersion::Absent) = (&upload.to, &upload.base) {
            return Err(self.changed_conflict(id, &upload.base).await);
        }
        let size = upload.content.size();
        let response = match size <= self.uploads.simple_max as u64 {
            true => {
                let bytes = upload.content.read(0, size).map_err(|_| again())?;
                let url = match &upload.to {
                    UploadTo::New(place) => {
                        let parent = self.parent_id(place).await?;
                        self.addr.child(
                            Some(&parent),
                            &place.name,
                            Some("content"),
                            "?@microsoft.graph.conflictBehavior=fail",
                        )
                    }
                    UploadTo::Replace(id) => self.addr.item(&id.0, Some("content"), ""),
                };
                let request = HttpRequest::new(Method::Put, url.ok_or(PutRefused::Forbidden)?)
                    .with_header("Content-Type", "application/octet-stream")
                    .with_body(bytes);
                let request = match guard_of(&upload.base) {
                    Some(tag) => request.with_header("If-Match", tag),
                    None => request,
                };
                let response = self.write_send(request).await?;
                upload.progress.sent(size);
                response
            }
            false => self.in_session(&upload).await?,
        };
        self.settle_upload(response, &upload.to, &upload.base).await
    }

    /// Where a session from before stands: the first byte it still wants, or `None` when the
    /// server no longer has it.
    async fn resumed(&self, session: &Session) -> Result<Option<(WebUrl, u64)>, PutRefused> {
        let Ok(url) = WebUrl::parse(session.address()) else {
            return Ok(None);
        };
        let response = self
            .write_send(HttpRequest::new(Method::Get, url.clone()))
            .await?;
        match response.status.0 {
            200 => {
                let status: SessionStatus =
                    serde_json::from_slice(&response.body).map_err(|_| again())?;
                Ok(status.from().map(|from| (url, from)))
            }
            404 | 410 => Ok(None),
            _ => Err(write_error(&response)),
        }
    }

    /// Opens an upload session for `upload`: its URL, or the refusal as the response.
    async fn open_session(
        &self,
        upload: &Upload<'_>,
    ) -> Result<Result<WebUrl, HttpResponse>, PutRefused> {
        let (url, behaviour) = match &upload.to {
            UploadTo::New(place) => {
                let parent = self.parent_id(place).await?;
                (
                    self.addr
                        .child(Some(&parent), &place.name, Some("createUploadSession"), ""),
                    "fail",
                )
            }
            UploadTo::Replace(id) => (
                self.addr.item(&id.0, Some("createUploadSession"), ""),
                "replace",
            ),
        };
        let body = format!(r#"{{"item":{{"@microsoft.graph.conflictBehavior":"{behaviour}"}}}}"#);
        let request = HttpRequest::new(Method::Post, url.ok_or(PutRefused::Forbidden)?)
            .with_header("Content-Type", "application/json")
            .with_body(body);
        let request = match guard_of(&upload.base) {
            Some(tag) => request.with_header("If-Match", tag),
            None => request,
        };
        let created = self.write_send(request).await?;
        if created.status.0 != 200 {
            return Ok(Err(created));
        }
        let session: SessionJson = serde_json::from_slice(&created.body).map_err(|_| again())?;
        let url = WebUrl::parse(&session.upload_url).map_err(|_| again())?;
        upload
            .progress
            .session(Some(&Session::new(url.as_str().to_owned())));
        Ok(Ok(url))
    }

    /// Sends `upload` in a session, resuming the one it names when the server still has it.
    async fn in_session(&self, upload: &Upload<'_>) -> Result<HttpResponse, PutRefused> {
        let resumed = match &upload.resume {
            Some(session) => self.resumed(session).await?,
            None => None,
        };
        let (url, mut sent) = match resumed {
            Some(found) => found,
            None => {
                if upload.resume.is_some() {
                    upload.progress.session(None);
                }
                match self.open_session(upload).await? {
                    Ok(url) => (url, 0),
                    Err(refusal) => return Ok(refusal),
                }
            }
        };
        let total = upload.content.size();
        upload.progress.sent(sent);
        while sent < total {
            let chunk = upload
                .content
                .read(sent, self.uploads.chunk as u64)
                .map_err(|_| again())?;
            let last = sent + chunk.len() as u64 - 1;
            let request = HttpRequest::new(Method::Put, url.clone())
                .with_header("Content-Type", "application/octet-stream")
                .with_header("Content-Range", format!("bytes {sent}-{last}/{total}"))
                .with_body(chunk);
            let response = match self.write_send(request).await {
                Ok(response) => response,
                // Not reached: the session stays, to be resumed.
                Err(why) => return Err(why),
            };
            match response.status.0 {
                202 => {
                    sent = last + 1;
                    upload.progress.sent(sent);
                }
                200 | 201 => {
                    upload.progress.sent(total);
                    upload.progress.session(None);
                    return Ok(response);
                }
                // Throttled, or the server failed: the session stays, to be resumed.
                429 | 500..=599 => return Ok(response),
                _ => {
                    // Give the session up; the server drops it anyway in time.
                    let _ = self
                        .write_send(HttpRequest::new(Method::Delete, url.clone()))
                        .await;
                    upload.progress.session(None);
                    return Ok(response);
                }
            }
        }
        Err(again())
    }

    async fn drive_make_folder(&self, place: &Place) -> Result<Written, PutRefused> {
        let parent = self.parent_id(place).await?;
        let url = self
            .addr
            .item(&parent, Some("children"), "")
            .ok_or(PutRefused::Forbidden)?;
        let body = format!(
            r#"{{"name":{},"folder":{{}},"@microsoft.graph.conflictBehavior":"fail"}}"#,
            json_string(&place.name)
        );
        let request = HttpRequest::new(Method::Post, url)
            .with_header("Content-Type", "application/json")
            .with_body(body);
        let response = self.write_send(request).await?;
        match response.status.0 {
            200 | 201 => {
                let (written, item) = self.written(&response)?;
                self.remember(&item);
                Ok(written)
            }
            409 => Err(self.taken_conflict(place).await),
            _ => Err(write_error(&response)),
        }
    }

    async fn drive_relocate(
        &self,
        item: &RemoteId,
        base: &BaseVersion,
        to: &Place,
    ) -> Result<Written, PutRefused> {
        let Some(tag) = guard_of(base) else {
            return Err(self.changed_conflict(item, base).await);
        };
        let parent = self.parent_id(to).await?;
        let url = self
            .addr
            .item(&item.0, None, "")
            .ok_or(PutRefused::Forbidden)?;
        let body = format!(
            r#"{{"name":{},"parentReference":{{"id":{}}}}}"#,
            json_string(&to.name),
            json_string(&parent)
        );
        let request = HttpRequest::new(Method::Patch, url)
            .with_header("Content-Type", "application/json")
            .with_header("If-Match", tag)
            .with_body(body);
        let response = self.write_send(request).await?;
        match response.status.0 {
            200 => {
                let (written, item) = self.written(&response)?;
                self.remember(&item);
                Ok(written)
            }
            404 | 412 => Err(self.changed_conflict(item, base).await),
            409 => Err(self.taken_conflict(to).await),
            _ => Err(write_error(&response)),
        }
    }

    async fn drive_trash(&self, item: &RemoteId, base: &BaseVersion) -> Result<(), PutRefused> {
        let Some(tag) = guard_of(base) else {
            return Err(self.changed_conflict(item, base).await);
        };
        let url = self
            .addr
            .item(&item.0, None, "")
            .ok_or(PutRefused::Forbidden)?;
        // OneDrive keeps a deleted item in the recycle bin.
        let request = HttpRequest::new(Method::Delete, url).with_header("If-Match", tag);
        let response = self.write_send(request).await?;
        match response.status.0 {
            200 | 204 => {
                self.state().folders.remove(item);
                Ok(())
            }
            404 | 412 => Err(self.changed_conflict(item, base).await),
            _ => Err(write_error(&response)),
        }
    }
}

impl<H: Http> DriveWrite for GraphReplica<H> {
    fn accepts_name(&self, name: &str) -> bool {
        accepts_name(name)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onedrive_refuses_the_names_microsoft_lists() {
        const CASES: &[(&str, bool)] = &[
            ("Report.docx", true),
            ("a.b.c", true),
            ("Com10", true),
            ("conference.txt", true),
            ("", false),
            ("a:b", false),
            ("a\"b", false),
            ("why?", false),
            ("a|b", false),
            ("a\\b", false),
            ("a/b", false),
            (" lead", false),
            ("trail ", false),
            ("dot.", false),
            ("CON", false),
            ("nul", false),
            ("COM3", false),
            ("lpt9", false),
            (".lock", false),
            ("Desktop.ini", false),
            ("~$Report.docx", false),
            ("x_vti_y", false),
        ];
        for (name, ok) in CASES {
            assert_eq!(accepts_name(name), *ok, "{name:?}");
        }
        assert!(accepts_name(&"a".repeat(255)));
        assert!(!accepts_name(&"a".repeat(256)));
    }
}
