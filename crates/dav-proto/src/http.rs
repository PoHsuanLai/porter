//! The requests as `http::Request<String>` (feature `http`): the bodies of the crate root with
//! their method, `Depth` and `Content-Type`. The URI is the caller's, already resolved.

use crate::{Depth, Multiget, SyncLevel};
use ::http::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use ::http::{Method, Request, Uri};

const XML: &str = "application/xml; charset=utf-8";

/// A PROPFIND of `props` on `uri` at `depth`.
pub fn propfind(uri: Uri, depth: Depth, props: &[&str]) -> Request<String> {
    xml(method(b"PROPFIND"), uri, depth, crate::propfind(props))
}

/// A sync-collection REPORT on `uri` from `token` (empty for the first listing).
pub fn sync_collection(uri: Uri, token: &str, level: SyncLevel, props: &[&str]) -> Request<String> {
    let body = crate::sync_collection(token, level, props);
    xml(method(b"REPORT"), uri, Depth::Zero, body)
}

/// A multiget REPORT on the collection at `uri` for `hrefs`.
pub fn multiget(uri: Uri, kind: Multiget, hrefs: &[&str]) -> Request<String> {
    xml(
        method(b"REPORT"),
        uri,
        Depth::One,
        crate::multiget(kind, hrefs),
    )
}

fn xml(method: Method, uri: Uri, depth: Depth, body: String) -> Request<String> {
    let mut request = Request::new(body);
    *request.method_mut() = method;
    *request.uri_mut() = uri;
    let headers = request.headers_mut();
    headers.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_static(depth.as_str()),
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(XML));
    request
}

/// An extension method. Both names used here are valid tokens, so this never fails.
fn method(name: &'static [u8]) -> Method {
    Method::from_bytes(name).unwrap_or(Method::GET)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::GETETAG;

    fn uri() -> Uri {
        Uri::from_static("https://dav.example.test/remote.php/dav/")
    }

    #[test]
    fn a_propfind_has_its_method_depth_and_type() {
        let request = propfind(uri(), Depth::One, &[GETETAG]);
        assert_eq!(request.method().as_str(), "PROPFIND");
        assert_eq!(request.uri(), &uri());
        assert_eq!(request.headers()["depth"], "1");
        assert_eq!(request.headers()[CONTENT_TYPE], XML);
        assert!(request.body().contains("<d:getetag/>"));
    }

    #[test]
    fn reports_are_depth_zero_for_sync_and_one_for_multiget() {
        let sync = sync_collection(uri(), "t", SyncLevel::One, &[GETETAG]);
        assert_eq!(sync.method().as_str(), "REPORT");
        assert_eq!(sync.headers()["depth"], "0");
        let get = multiget(uri(), Multiget::Addressbook, &["/a.vcf"]);
        assert_eq!(get.method().as_str(), "REPORT");
        assert_eq!(get.headers()["depth"], "1");
        assert!(get.body().contains("addressbook-multiget"));
    }
}
