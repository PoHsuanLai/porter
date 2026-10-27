//! The requests: PROPFIND and the sync-collection REPORT, as values of the HTTP seam. The
//! bodies are dav-proto's.

use dav_proto::{Depth, SyncLevel};
use porter_core::WebUrl;
use porter_http::http::header::CONTENT_TYPE;
use porter_http::http::{Method, Request};
use porter_http::{HttpError, request, webdav};

const XML: &str = "application/xml; charset=utf-8";

/// A PROPFIND of `props` (namespace-qualified names) on `url`. `Malformed` when the URL is not
/// one the `http` crate takes as a URI.
pub fn propfind(url: WebUrl, depth: Depth, props: &[&str]) -> Result<Request<Vec<u8>>, HttpError> {
    xml(webdav::propfind(), url, depth, dav_proto::propfind(props))
}

/// A sync-collection REPORT on `url` from `sync_token` (empty for the first listing) at
/// `level`, asking for `props` of each changed member. `Malformed` when the URL is not one the
/// `http` crate takes as a URI.
pub fn report_sync_collection(
    url: WebUrl,
    sync_token: &str,
    level: SyncLevel,
    props: &[&str],
) -> Result<Request<Vec<u8>>, HttpError> {
    let body = dav_proto::sync_collection(sync_token, level, props);
    xml(webdav::report(), url, Depth::Zero, body)
}

fn xml(
    method: Method,
    url: WebUrl,
    depth: Depth,
    body: String,
) -> Result<Request<Vec<u8>>, HttpError> {
    let request = request(method, &url)
        .header("Depth", depth.as_str())
        .header(CONTENT_TYPE, XML)
        .body(body.into_bytes())?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dav_proto::names::*;

    fn url() -> WebUrl {
        WebUrl::parse("https://dav.example.test/remote.php/dav/").unwrap()
    }

    fn body(request: &Request<Vec<u8>>) -> String {
        String::from_utf8(request.body().clone()).unwrap()
    }

    fn header<'a>(request: &'a Request<Vec<u8>>, name: &str) -> Option<&'a str> {
        request.headers().get(name)?.to_str().ok()
    }

    #[test]
    fn a_propfind_carries_its_method_depth_and_body() {
        let request = propfind(
            url(),
            Depth::Zero,
            &[CURRENT_USER_PRINCIPAL, ADDRESSBOOK_HOME_SET],
        )
        .unwrap();
        assert_eq!(request.method(), "PROPFIND");
        assert_eq!(header(&request, "depth"), Some("0"));
        assert_eq!(
            header(&request, "content-type"),
            Some("application/xml; charset=utf-8")
        );
        let text = body(&request);
        let doc = roxmltree::Document::parse(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
        assert!(
            doc.descendants()
                .any(|n| n.has_tag_name((CARDDAV, "addressbook-home-set")))
        );
    }

    #[test]
    fn the_depth_header_follows_the_depth() {
        assert_eq!(
            header(&propfind(url(), Depth::One, &[GETETAG]).unwrap(), "depth"),
            Some("1")
        );
    }

    #[test]
    fn a_sync_report_is_depth_zero_and_carries_its_token_and_level() {
        let request = report_sync_collection(
            url(),
            "http://x.test/sync?a=1&b=<2>",
            SyncLevel::One,
            &[GETETAG],
        )
        .unwrap();
        assert_eq!(request.method(), "REPORT");
        assert_eq!(header(&request, "depth"), Some("0"));
        let text = body(&request);
        let doc = roxmltree::Document::parse(&text).unwrap();
        let token = doc
            .descendants()
            .find(|n| n.has_tag_name((DAV, "sync-token")))
            .and_then(|n| n.text());
        assert_eq!(token, Some("http://x.test/sync?a=1&b=<2>"));
        assert!(text.contains("<d:sync-level>1</d:sync-level>"), "{text}");
        assert!(text.contains("<d:getetag/>"), "{text}");
    }
}
