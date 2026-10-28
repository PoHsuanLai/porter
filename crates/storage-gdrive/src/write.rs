//! Writes: `put` and `remove`. Drive documents no conditional update, so each write is checked
//! first: a new file looks for its name in its parent (a name already there is a `Conflict`), a
//! changed file or a removal reads the file and compares its `version` with the base (a stale
//! base is a `Conflict`, never an overwrite). The check and the write are two requests, so
//! another writer can slip in between: the window is one round trip. A refusal is read back by
//! asking the server what is there now.

use crate::addr::{FILE_FIELDS, names_of};
use crate::json::File;
use crate::refuse::write_error;
use crate::replica::GdriveReplica;
use porter_http::http::header::{ACCEPT, CONTENT_TYPE, HeaderValue};
use porter_http::http::{Method, Request, Response};
use porter_http::{Http, request};
use porter_sync::DELETED;
use porter_sync::status::{refused, write_unreached};
use porter_sync::{
    BaseVersion, Conflict, PutItem, PutRefused, PutTarget, RemoteId, RemoteSide, RemoteVersion,
    RetryAfter,
};

/// Where a write goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// A new file in this folder, with this name.
    New { parent: String, name: String },
    /// A file that exists.
    Existing(RemoteId),
}

type Written = Result<(RemoteId, RemoteVersion), PutRefused>;

/// A boundary that does not occur in `content`.
fn boundary_for(content: &[u8]) -> String {
    let occurs = |b: &str| {
        let needle = b.as_bytes();
        content.windows(needle.len()).any(|w| w == needle)
    };
    // Every occurrence rules out one boundary, so any content under 4 GiB leaves one free.
    (0..=u32::MAX)
        .map(|n| format!("porter-gdrive-{n:08x}"))
        .find(|b| !occurs(b))
        .unwrap_or_default()
}

/// The body of a `multipart/related` request: the metadata, then the content.
pub(crate) fn multipart(meta: &str, content: &[u8]) -> (String, Vec<u8>) {
    let boundary = boundary_for(content);
    let mut body = format!(
        "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--").as_bytes());
    (boundary, body)
}

impl<H: Http> GdriveReplica<H> {
    pub(crate) async fn write(&self, item: PutItem, base: BaseVersion) -> Written {
        let bytes = item.content.0;
        let small = bytes.len() <= self.uploads.simple_max;
        let (target, response) = match item.target {
            PutTarget::New(path) => {
                let names = names_of(&path.0).ok_or(PutRefused::Forbidden)?;
                let (name, folders) = names.split_last().ok_or(PutRefused::Forbidden)?;
                let root = self.root_id(true).await.map_err(refused)?;
                let parent = self
                    .folder_below(&root, folders, true)
                    .await
                    .map_err(refused)?;
                let target = Target::New {
                    parent: parent.clone(),
                    name: name.clone(),
                };
                let there = self.named_in(&parent, name).await.map_err(refused)?;
                if there.iter().any(File::is_folder) && !there.iter().any(|f| !f.is_folder()) {
                    // A folder is where the file would go.
                    return Err(PutRefused::Forbidden);
                }
                if let Some(found) = there.iter().find(|f| !f.is_folder()) {
                    return Err(Self::conflict(&target, &base, Some(found)));
                }
                let meta = serde_json::json!({"name": name, "parents": [parent]}).to_string();
                let response = match small {
                    true => self.create_small(&meta, bytes).await?,
                    false => self.put_session(None, &meta, bytes).await?,
                };
                (target, response)
            }
            PutTarget::Existing(id) => {
                let target = Target::Existing(id.clone());
                let BaseVersion::At(seen) = &base else {
                    // An existing file written on the belief that it is absent: it is not.
                    let now = self.file_now(&id.0).await.map_err(refused)?;
                    return Err(Self::conflict(&target, &base, now.as_ref()));
                };
                let now = self.file_now(&id.0).await.map_err(refused)?;
                match &now {
                    Some(file)
                        if !file.in_trash() && file.version.as_deref() == Some(seen.0.as_str()) => {
                    }
                    _ => return Err(Self::conflict(&target, &base, now.as_ref())),
                }
                let response = match small {
                    true => self.update_small(&id.0, bytes).await?,
                    false => self.put_session(Some(&id.0), "{}", bytes).await?,
                };
                (target, response)
            }
        };
        self.settle(response, &target, &base).await
    }

    pub(crate) async fn delete(
        &self,
        item: &RemoteId,
        base: BaseVersion,
    ) -> Result<RemoteVersion, PutRefused> {
        let target = Target::Existing(item.clone());
        let now = self.file_now(&item.0).await.map_err(refused)?;
        let BaseVersion::At(seen) = &base else {
            return Err(Self::conflict(&target, &base, now.as_ref()));
        };
        match &now {
            Some(file) if !file.in_trash() && file.version.as_deref() == Some(seen.0.as_str()) => {}
            _ => return Err(Self::conflict(&target, &base, now.as_ref())),
        }
        let url = self.addr.file(&item.0, &[]).ok_or(PutRefused::Forbidden)?;
        let delete = request(Method::DELETE, &url)
            .body(Vec::new())
            .map_err(|e| write_unreached(e.into()))?;
        let response = self.write_send(delete).await?;
        match response.status().as_u16() {
            200 | 204 => {
                self.state().folders.remove_file(item);
                Ok(RemoteVersion(DELETED.to_owned()))
            }
            404 => Err(Self::conflict(&target, &base, None)),
            _ => Err(write_error(&response, self.clock.now())),
        }
    }

    pub(crate) async fn write_send(
        &self,
        mut request: Request<Vec<u8>>,
    ) -> Result<Response<Vec<u8>>, PutRefused> {
        request
            .headers_mut()
            .append(ACCEPT, HeaderValue::from_static("application/json"));
        self.http.send(request).await.map_err(write_unreached)
    }

    /// A new file, metadata and content in one request.
    pub(crate) async fn create_small(
        &self,
        meta: &str,
        bytes: Vec<u8>,
    ) -> Result<Response<Vec<u8>>, PutRefused> {
        let url = self
            .addr
            .upload(
                "files",
                &[("uploadType", "multipart"), ("fields", FILE_FIELDS)],
            )
            .ok_or(PutRefused::Forbidden)?;
        let (boundary, body) = multipart(meta, &bytes);
        let create = request(Method::POST, &url)
            .header(
                CONTENT_TYPE,
                format!("multipart/related; boundary={boundary}"),
            )
            .body(body)
            .map_err(|e| write_unreached(e.into()))?;
        self.write_send(create).await
    }

    /// New content for a file, in one request. Google's APIs take a POST with
    /// `X-HTTP-Method-Override: PATCH` where a client has no PATCH.
    pub(crate) async fn update_small(
        &self,
        id: &str,
        bytes: Vec<u8>,
    ) -> Result<Response<Vec<u8>>, PutRefused> {
        let path = format!("files/{}", porter_ids::percent::encode_unreserved(id));
        let url = self
            .addr
            .upload(&path, &[("uploadType", "media"), ("fields", FILE_FIELDS)])
            .ok_or(PutRefused::Forbidden)?;
        let update = request(Method::POST, &url)
            .header("X-HTTP-Method-Override", "PATCH")
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .map_err(|e| write_unreached(e.into()))?;
        self.write_send(update).await
    }

    /// What a finished write's answer comes to.
    pub(crate) async fn settle(
        &self,
        response: Response<Vec<u8>>,
        target: &Target,
        base: &BaseVersion,
    ) -> Written {
        match response.status().as_u16() {
            200 | 201 => {
                let file: File = serde_json::from_slice(response.body())
                    .map_err(|_| PutRefused::Transient(RetryAfter(30)))?;
                if let Some(parent) = file.parent_id() {
                    self.state()
                        .folders
                        .insert_file(RemoteId(file.id.clone()), RemoteId(parent.to_owned()));
                }
                Ok((
                    RemoteId(file.id),
                    RemoteVersion(file.version.unwrap_or_default()),
                ))
            }
            404 => match target {
                Target::Existing(id) => {
                    let now = self.file_now(&id.0).await.map_err(refused)?;
                    Err(Self::conflict(target, base, now.as_ref()))
                }
                Target::New { .. } => {
                    // The folder it was going into is gone: ask again from the top.
                    *self.state() = crate::replica::State::default();
                    Err(PutRefused::Transient(RetryAfter(1)))
                }
            },
            _ => Err(write_error(&response, self.clock.now())),
        }
    }

    /// The conflict a refused write is: what the server has there now.
    pub(crate) fn conflict(target: &Target, base: &BaseVersion, now: Option<&File>) -> PutRefused {
        let live = now.filter(|f| !f.in_trash());
        let version = |f: &File| RemoteVersion(f.version.clone().unwrap_or_default());
        let (item, remote) = match (live, base) {
            (Some(f), BaseVersion::Absent) => (
                RemoteId(f.id.clone()),
                RemoteSide::Exists(RemoteId(f.id.clone()), version(f)),
            ),
            (Some(f), BaseVersion::At(_)) => {
                (RemoteId(f.id.clone()), RemoteSide::Changed(version(f)))
            }
            (None, BaseVersion::At(_)) => match target {
                Target::Existing(id) => (
                    id.clone(),
                    RemoteSide::Deleted(RemoteVersion(DELETED.to_owned())),
                ),
                Target::New { .. } => return PutRefused::Transient(RetryAfter(1)),
            },
            (None, BaseVersion::Absent) => return PutRefused::Transient(RetryAfter(1)),
        };
        PutRefused::Conflict(Conflict {
            item,
            base: base.clone(),
            remote,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_boundary_never_occurs_in_the_content() {
        let first = boundary_for(b"plain");
        assert_eq!(first, "porter-gdrive-00000000");
        let taken = boundary_for(first.as_bytes());
        assert_ne!(taken, first);
        assert!(
            !first
                .as_bytes()
                .windows(taken.len())
                .any(|w| w == taken.as_bytes())
        );
    }

    #[test]
    fn a_multipart_body_is_the_metadata_then_the_content() {
        let (boundary, body) = multipart(r#"{"name":"a"}"#, b"\x00bytes\r\n");
        let text = String::from_utf8_lossy(&body).into_owned();
        assert!(text.starts_with(&format!("--{boundary}\r\nContent-Type: application/json")));
        assert!(text.contains("\r\n\r\n{\"name\":\"a\"}\r\n--"));
        assert!(text.ends_with(&format!("\r\n--{boundary}--")));
        assert!(body.windows(8).any(|w| w == b"\x00bytes\r\n"));
    }
}
