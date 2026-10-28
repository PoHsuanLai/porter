//! The requests the replica sends, as values. Each is `Malformed` when the URL is not one the
//! `http` crate takes as a URI.

use crate::entry::CONTENT_LENGTH;
use dav_proto::http::bytes;
use dav_proto::{Depth, SyncLevel, names};
use porter_http::http::Request;
use porter_http::{HttpError, uri};
use porter_ids::WebUrl;

fn propfind(url: WebUrl, depth: Depth, props: &[&str]) -> Result<Request<Vec<u8>>, HttpError> {
    Ok(bytes::propfind(uri(&url)?, depth, props))
}

/// A PROPFIND at depth 1 of what a listing needs.
pub fn list(url: WebUrl) -> Result<Request<Vec<u8>>, HttpError> {
    propfind(
        url,
        Depth::One,
        &[names::GETETAG, names::RESOURCETYPE, CONTENT_LENGTH],
    )
}

/// A PROPFIND at depth 0 of what a listing needs, for one resource.
pub fn stat(url: WebUrl) -> Result<Request<Vec<u8>>, HttpError> {
    propfind(
        url,
        Depth::Zero,
        &[names::GETETAG, names::RESOURCETYPE, CONTENT_LENGTH],
    )
}

/// A PROPFIND of the quota properties (RFC 4331).
pub fn quota(url: WebUrl) -> Result<Request<Vec<u8>>, HttpError> {
    propfind(
        url,
        Depth::Zero,
        &[names::QUOTA_USED, names::QUOTA_AVAILABLE],
    )
}

/// A sync-collection REPORT from `token` (empty for the first listing) over everything below
/// the folder (RFC 6578 section 3.3, `sync-level` `infinite`), asking for what a listing reads.
pub fn sync(url: WebUrl, token: &str) -> Result<Request<Vec<u8>>, HttpError> {
    Ok(bytes::sync_collection(
        uri(&url)?,
        token,
        SyncLevel::Infinite,
        &[names::GETETAG, CONTENT_LENGTH, names::RESOURCETYPE],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_asks_for_every_level_and_what_a_listing_reads_and_escapes_the_token() {
        let url = WebUrl::parse("http://127.0.0.1:1/dav/").expect("url");
        let request = sync(url, "t&<1>").expect("request");
        let text = String::from_utf8(request.into_body()).expect("utf8");
        let doc = roxmltree::Document::parse(&text).expect("xml");
        let text_of = |name: &str| {
            doc.descendants()
                .find(|n| n.tag_name().name() == name)
                .and_then(|n| n.text())
                .map(str::to_owned)
        };
        assert_eq!(text_of("sync-token").as_deref(), Some("t&<1>"));
        assert_eq!(text_of("sync-level").as_deref(), Some("infinite"));
        for prop in ["getetag", "getcontentlength", "resourcetype"] {
            assert!(
                doc.descendants().any(|n| n.tag_name().name() == prop),
                "{prop}"
            );
        }
    }
}
