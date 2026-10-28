//! Large files: a resumable upload. The first request (metadata only) answers `200` with the
//! session's URL in `Location`; the content goes to it in chunks (each but the last a multiple of
//! 256 KiB) with a `Content-Range`, `308` says send the next, `200` or `201` carries the
//! finished file.
//!
//! The session URL is pre-authorised by Google but on the API's own origin, so it goes through
//! the same `Http` as everything else (in syncd the relay that adds the bearer); a URL on
//! another origin is refused rather than followed.

use crate::addr::FILE_FIELDS;
use crate::feed::link;
use crate::replica::GdriveReplica;
use porter_http::http::header::CONTENT_TYPE;
use porter_http::http::{Method, Response};
use porter_http::{HeaderText, Http, request};
use porter_sync::status::write_unreached;
use porter_sync::{PutRefused, RetryAfter};

impl<H: Http> GdriveReplica<H> {
    /// Sends `bytes` in a resumable session: a new file described by `meta`, or new content for
    /// the file `over`. A refusal at any step is returned as the response, for `settle` to
    /// class; a session that cannot be run at all is `Transient`.
    pub(crate) async fn put_session(
        &self,
        over: Option<&str>,
        meta: &str,
        bytes: Vec<u8>,
    ) -> Result<Response<Vec<u8>>, PutRefused> {
        let again = || PutRefused::Transient(RetryAfter(30));
        let pairs = [("uploadType", "resumable"), ("fields", FILE_FIELDS)];
        let start = match over {
            None => self.addr.upload("files", &pairs),
            Some(id) => self.addr.upload(
                &format!("files/{}", porter_ids::percent::encode_unreserved(id)),
                &pairs,
            ),
        };
        let mut open = request(Method::POST, &start.ok_or(PutRefused::Forbidden)?)
            .header(CONTENT_TYPE, "application/json; charset=UTF-8")
            .header("X-Upload-Content-Type", "application/octet-stream")
            .header("X-Upload-Content-Length", bytes.len().to_string());
        if over.is_some() {
            open = open.header("X-HTTP-Method-Override", "PATCH");
        }
        let open = open
            .body(meta.as_bytes().to_vec())
            .map_err(|e| write_unreached(e.into()))?;
        let created = self.write_send(open).await?;
        if created.status().as_u16() != 200 {
            return Ok(created);
        }
        let session = created
            .header("location")
            .and_then(link)
            .filter(|url| Some(url.origin()) == self.addr.origin())
            .ok_or_else(again)?;
        let total = bytes.len();
        let mut sent = 0;
        for chunk in bytes.chunks(self.uploads.chunk) {
            let last = sent + chunk.len() - 1;
            let piece = request(Method::PUT, &session)
                .header(CONTENT_TYPE, "application/octet-stream")
                .header("Content-Range", format!("bytes {sent}-{last}/{total}"))
                .body(chunk.to_vec())
                .map_err(|e| write_unreached(e.into()))?;
            let response = self.write_send(piece).await?;
            sent += chunk.len();
            if response.status().as_u16() != 308 {
                return Ok(response);
            }
        }
        Err(again())
    }
}
