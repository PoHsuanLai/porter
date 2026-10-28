//! Large files: an upload session. `createUploadSession` answers a URL, the content goes to it in
//! chunks (each but the last a multiple of 320 KiB) with a `Content-Range`, `202` says send the
//! next, `200` or `201` carries the finished item. The conditions travel with the session: `fail`
//! for a new item, `If-Match` for an existing one, and the server checks them again at the end.
//!
//! The session URL is pre-authenticated and on another host in Graph; it is sent through the same
//! `Http` as everything else, which routes a request for another origin to a stream that adds no
//! credential (`porter_http_relay::Routed`).

use crate::json::Session;
use crate::replica::GraphReplica;
use crate::write::Target;
use porter_http::http::header::CONTENT_TYPE;
use porter_http::http::{Method, Response};
use porter_http::{Http, request};
use porter_ids::WebUrl;
use porter_sync::status::write_unreached;
use porter_sync::{PutRefused, RetryAfter};

impl<H: Http> GraphReplica<H> {
    /// Sends `bytes` in an upload session. A refusal at any step is returned as the response, for
    /// `settle` to class; a session that cannot be run at all is `Transient`.
    pub(crate) async fn put_session(
        &self,
        target: &Target,
        guard: Option<&str>,
        bytes: Vec<u8>,
    ) -> Result<Response<Vec<u8>>, PutRefused> {
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
        let create = request(Method::POST, &url.ok_or(PutRefused::Forbidden)?)
            .header(CONTENT_TYPE, "application/json");
        let create = match guard {
            Some(tag) => create.header("If-Match", tag),
            None => create,
        };
        let create = create
            .body(body.into_bytes())
            .map_err(|e| write_unreached(e.into()))?;
        let created = self.write_send(create).await?;
        if created.status().as_u16() != 200 {
            return Ok(created);
        }
        let again = || PutRefused::Transient(RetryAfter(30));
        let session: Session = serde_json::from_slice(created.body()).map_err(|_| again())?;
        let upload = WebUrl::parse(&session.upload_url).map_err(|_| again())?;
        let total = bytes.len();
        let mut sent = 0;
        for chunk in bytes.chunks(self.uploads.chunk) {
            let last = sent + chunk.len() - 1;
            let piece = request(Method::PUT, &upload)
                .header(CONTENT_TYPE, "application/octet-stream")
                .header("Content-Range", format!("bytes {sent}-{last}/{total}"))
                .body(chunk.to_vec())
                .map_err(|e| write_unreached(e.into()))?;
            let response = self.write_send(piece).await?;
            sent += chunk.len();
            if response.status().as_u16() != 202 {
                if !matches!(response.status().as_u16(), 200 | 201) {
                    // Give the session up; the server drops it anyway in time.
                    if let Ok(give_up) = request(Method::DELETE, &upload).body(Vec::new()) {
                        let _ = self.write_send(give_up).await;
                    }
                }
                return Ok(response);
            }
        }
        Err(again())
    }
}
