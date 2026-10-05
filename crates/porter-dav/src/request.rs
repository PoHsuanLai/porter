//! The requests: PROPFIND and the sync-collection REPORT.

use porter_core::EndpointUrl;
use porter_http::HttpRequest;

/// How deep a PROPFIND goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// The resource itself.
    Zero,
    /// Its members.
    One,
}

/// A PROPFIND of `props` (namespace-qualified names) on `url`.
pub fn propfind(url: EndpointUrl, depth: Depth, props: &[&str]) -> HttpRequest {
    let _ = (url, depth, props);
    todo!("method PROPFIND, a `Depth` header, an XML body naming the properties")
}

/// A sync-collection REPORT on `url` from `sync_token` (empty for the first listing).
pub fn report_sync_collection(url: EndpointUrl, sync_token: &str) -> HttpRequest {
    let _ = (url, sync_token);
    todo!("method REPORT, `Depth: 0`, a sync-collection body with the token and getetag")
}
