//! The requests: PROPFIND and the sync-collection REPORT, as values of the HTTP seam. The
//! bodies are dav-proto's.

use dav_proto::{Depth, SyncLevel};
use porter_core::WebUrl;
use porter_http::{HttpRequest, Method};

const XML: &str = "application/xml; charset=utf-8";

/// A PROPFIND of `props` (namespace-qualified names) on `url`.
pub fn propfind(url: WebUrl, depth: Depth, props: &[&str]) -> HttpRequest {
    xml(Method::Propfind, url, depth, dav_proto::propfind(props))
}

/// A sync-collection REPORT on `url` from `sync_token` (empty for the first listing) at
/// `level`, asking for `props` of each changed member.
pub fn report_sync_collection(
    url: WebUrl,
    sync_token: &str,
    level: SyncLevel,
    props: &[&str],
) -> HttpRequest {
    let body = dav_proto::sync_collection(sync_token, level, props);
    xml(Method::Report, url, Depth::Zero, body)
}

fn xml(method: Method, url: WebUrl, depth: Depth, body: String) -> HttpRequest {
    HttpRequest::new(method, url)
        .with_header("Depth", depth.as_str())
        .with_header("Content-Type", XML)
        .with_body(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dav_proto::names::*;

    fn url() -> WebUrl {
        WebUrl::parse("https://dav.example.test/remote.php/dav/").unwrap()
    }

    fn body(request: &HttpRequest) -> String {
        String::from_utf8(request.body.clone()).unwrap()
    }

    fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
        request
            .headers
            .iter()
            .find(|h| h.name.as_str() == name)
            .map(|h| h.value.0.as_str())
    }

    #[test]
    fn a_propfind_carries_its_method_depth_and_body() {
        let request = propfind(
            url(),
            Depth::Zero,
            &[CURRENT_USER_PRINCIPAL, ADDRESSBOOK_HOME_SET],
        );
        assert_eq!(request.method, Method::Propfind);
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
            header(&propfind(url(), Depth::One, &[GETETAG]), "depth"),
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
        );
        assert_eq!(request.method, Method::Report);
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
