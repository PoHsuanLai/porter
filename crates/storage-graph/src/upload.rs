//! Large files: an upload session. `createUploadSession` answers a URL, the content goes to it in
//! chunks (each but the last a multiple of 320 KiB) with a `Content-Range`, `202` says send the
//! next, `200` or `201` carries the finished item. The conditions travel with the session: `fail`
//! for a new item, `If-Match` for an existing one, and the server checks them again at the end.
//!
//! The session URL is pre-authenticated and on another host in Graph; here it is sent through
//! the same `Http` as everything else (FINDINGS: the relay reaches one origin).

use crate::json::Session;
use crate::replica::GraphReplica;
use crate::write::Target;
use porter_core::WebUrl;
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use porter_sync::{PutRefused, RetryAfter};

impl<H: Http> GraphReplica<H> {
    /// Sends `bytes` in an upload session. A refusal at any step is returned as the response, for
    /// `settle` to class; a session that cannot be run at all is `Transient`.
    pub(crate) async fn put_session(
        &self,
        target: &Target,
        guard: Option<&str>,
        bytes: Vec<u8>,
    ) -> Result<HttpResponse, PutRefused> {
        let (url, behaviour) = match target {
            Target::New(names) => (
                self.addr.path(Some(names), Some("createUploadSession"), ""),
                "fail",
            ),
            Target::Existing(id) => (
                self.addr.item(&id.0, Some("createUploadSession"), ""),
                "replace",
            ),
        };
        let body = format!(r#"{{"item":{{"@microsoft.graph.conflictBehavior":"{behaviour}"}}}}"#);
        let request = HttpRequest::new(Method::Post, url.ok_or(PutRefused::Forbidden)?)
            .with_header("Content-Type", "application/json")
            .with_body(body);
        let request = match guard {
            Some(tag) => request.with_header("If-Match", tag),
            None => request,
        };
        let created = self.write_send(request).await?;
        if created.status.0 != 200 {
            return Ok(created);
        }
        let again = || PutRefused::Transient(RetryAfter(30));
        let session: Session = serde_json::from_slice(&created.body).map_err(|_| again())?;
        let upload = WebUrl::parse(&session.upload_url).map_err(|_| again())?;
        let total = bytes.len();
        let mut sent = 0;
        for chunk in bytes.chunks(self.uploads.chunk) {
            let last = sent + chunk.len() - 1;
            let request = HttpRequest::new(Method::Put, upload.clone())
                .with_header("Content-Type", "application/octet-stream")
                .with_header("Content-Range", format!("bytes {sent}-{last}/{total}"))
                .with_body(chunk.to_vec());
            let response = self.write_send(request).await?;
            sent += chunk.len();
            if response.status.0 != 202 {
                if !matches!(response.status.0, 200 | 201) {
                    // Give the session up; the server drops it anyway in time.
                    let _ = self
                        .write_send(HttpRequest::new(Method::Delete, upload.clone()))
                        .await;
                }
                return Ok(response);
            }
        }
        Err(again())
    }
}
