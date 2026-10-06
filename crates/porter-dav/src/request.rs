//! The requests: PROPFIND and the sync-collection REPORT.
//!
//! Adapted from mailo `crates/mail-pim/src/dav/request.rs` (same author, MIT OR Apache-2.0): the
//! body building and escaping; the vocabulary is generalised from CardDAV to any property name.

use crate::names::split;
use porter_core::WebUrl;
use porter_http::{HttpRequest, Method};

const HEAD: &str = r#"<?xml version="1.0" encoding="utf-8"?>"#;
const XML: &str = "application/xml; charset=utf-8";

/// How deep a PROPFIND goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// The resource itself.
    Zero,
    /// Its members.
    One,
}

impl Depth {
    fn header(self) -> &'static str {
        match self {
            Depth::Zero => "0",
            Depth::One => "1",
        }
    }
}

/// A PROPFIND of `props` (namespace-qualified names) on `url`.
pub fn propfind(url: WebUrl, depth: Depth, props: &[&str]) -> HttpRequest {
    // Each property gets its own namespace declaration: names are free-form, so no prefix table.
    let asked: String = props
        .iter()
        .map(|name| match split(name) {
            ("", local) => format!("<{}/>", escape(local)),
            (ns, local) => format!(r#"<p:{} xmlns:p="{}"/>"#, escape(local), escape(ns)),
        })
        .collect();
    let body = format!(r#"{HEAD}<d:propfind xmlns:d="DAV:"><d:prop>{asked}</d:prop></d:propfind>"#);
    xml(Method::Propfind, url, depth, body)
}

/// A sync-collection REPORT on `url` from `sync_token` (empty for the first listing).
pub fn report_sync_collection(url: WebUrl, sync_token: &str) -> HttpRequest {
    let token = escape(sync_token);
    let body = format!(
        r#"{HEAD}<d:sync-collection xmlns:d="DAV:"><d:sync-token>{token}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"#
    );
    xml(Method::Report, url, Depth::Zero, body)
}

fn xml(method: Method, url: WebUrl, depth: Depth, body: String) -> HttpRequest {
    let mut request = HttpRequest::new(method, url)
        .with_header("Depth", depth.header())
        .with_header("Content-Type", XML);
    request.body = body.into_bytes();
    request
}

/// XML character escaping for text and attribute content.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::*;

    const DAV: &str = "DAV:";

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
    fn a_propfind_names_each_property_in_its_namespace() {
        let request = propfind(
            url(),
            Depth::Zero,
            &[
                CURRENT_USER_PRINCIPAL,
                ADDRESSBOOK_HOME_SET,
                CALENDAR_HOME_SET,
                QUOTA_USED,
            ],
        );
        assert_eq!(request.method, Method::Propfind);
        assert_eq!(header(&request, "depth"), Some("0"));
        let text = body(&request);
        let doc = roxmltree::Document::parse(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
        let asked: Vec<(Option<&str>, &str)> = doc
            .descendants()
            .filter(|n| n.parent().is_some_and(|p| p.has_tag_name((DAV, "prop"))))
            .map(|n| (n.tag_name().namespace(), n.tag_name().name()))
            .collect();
        assert_eq!(
            asked,
            [
                (Some(DAV), "current-user-principal"),
                (
                    Some("urn:ietf:params:xml:ns:carddav"),
                    "addressbook-home-set"
                ),
                (Some("urn:ietf:params:xml:ns:caldav"), "calendar-home-set"),
                (Some(DAV), "quota-used-bytes"),
            ]
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
    fn a_sync_token_is_escaped_into_the_body() {
        let request = report_sync_collection(url(), "http://x.test/sync?a=1&b=<2>");
        assert_eq!(request.method, Method::Report);
        assert_eq!(header(&request, "depth"), Some("0"));
        let text = body(&request);
        let doc = roxmltree::Document::parse(&text).unwrap();
        let token = doc
            .descendants()
            .find(|n| n.has_tag_name((DAV, "sync-token")))
            .and_then(|n| n.text());
        assert_eq!(token, Some("http://x.test/sync?a=1&b=<2>"));
    }

    #[test]
    fn a_first_sync_sends_an_empty_token() {
        let text = body(&report_sync_collection(url(), ""));
        assert!(text.contains("<d:sync-token></d:sync-token>"), "{text}");
        assert!(text.contains("<d:getetag/>"));
    }
}
