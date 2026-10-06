//! The requests the replica sends, as values.
//!
//! The sync-collection REPORT is built here, not by `porter_dav::report_sync_collection`: a
//! replica needs `sync-level` `infinite` (the dataset has folders) and the size and resource
//! type of what changed, and that function asks for the etag at level 1 (an interface ask).

use crate::entry::CONTENT_LENGTH;
use porter_core::WebUrl;
use porter_dav::{Depth, names, propfind};
use porter_http::{HttpRequest, Method};

const XML: &str = "application/xml; charset=utf-8";

/// A PROPFIND at depth 1 of what a listing needs.
pub fn list(url: WebUrl) -> HttpRequest {
    propfind(
        url,
        Depth::One,
        &[names::GETETAG, names::RESOURCETYPE, CONTENT_LENGTH],
    )
}

/// A PROPFIND at depth 0 of what a listing needs, for one resource.
pub fn stat(url: WebUrl) -> HttpRequest {
    propfind(
        url,
        Depth::Zero,
        &[names::GETETAG, names::RESOURCETYPE, CONTENT_LENGTH],
    )
}

/// A PROPFIND of the quota properties (RFC 4331).
pub fn quota(url: WebUrl) -> HttpRequest {
    propfind(
        url,
        Depth::Zero,
        &[names::QUOTA_USED, names::QUOTA_AVAILABLE],
    )
}

/// A sync-collection REPORT from `token` (empty for the first listing) over everything below
/// the folder (RFC 6578 section 3.3, `sync-level` `infinite`).
pub fn sync(url: WebUrl, token: &str) -> HttpRequest {
    let token = token
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let body = format!(
        concat!(
            r#"<?xml version="1.0" encoding="utf-8"?>"#,
            r#"<d:sync-collection xmlns:d="DAV:"><d:sync-token>{}</d:sync-token>"#,
            r#"<d:sync-level>infinite</d:sync-level>"#,
            r#"<d:prop><d:getetag/><d:getcontentlength/><d:resourcetype/></d:prop></d:sync-collection>"#
        ),
        token
    );
    HttpRequest::new(Method::Report, url)
        .with_header("Depth", "0")
        .with_header("Content-Type", XML)
        .with_body(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_asks_for_every_level_and_what_a_listing_reads_and_escapes_the_token() {
        let url = WebUrl::parse("http://127.0.0.1:1/dav/").expect("url");
        let request = sync(url, "t&<1>");
        let text = String::from_utf8(request.body).expect("utf8");
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
