//! A `207 Multi-Status` body (RFC 4918 section 13).
//!
//! Adapted from mailo `crates/mail-pim/src/dav/reply.rs` (same author, MIT OR Apache-2.0): the
//! namespace-matched reading, `propstat` status handling and the DOCTYPE refusal. mailo's typed
//! CardDAV properties become named [`Prop`]s here.

use crate::names::qualify;
use roxmltree::{Document, Node};
use thiserror::Error;

const DAV: &str = "DAV:";

/// One property and the status it came with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    /// The property's name, namespace-qualified (`DAV:getetag`).
    pub name: String,
    /// Its text, empty for a property with no value.
    pub value: String,
    /// The status of the property's group.
    pub status: PropStatus,
}

/// The status of a property group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropStatus {
    /// 2xx.
    Found,
    /// 404: the server does not have it.
    Missing,
    /// Another status.
    Other(u16),
}

/// One `response` element: an href and its properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The resource, as written (a path or a URL).
    pub href: String,
    /// The status of the response as a whole, where it has one instead of per-property ones
    /// (in a sync-collection reply, 404 reports a removed member).
    pub status: Option<u16>,
    /// Its properties.
    pub props: Vec<Prop>,
}

/// A parsed multistatus.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Multistatus {
    /// The responses, in order.
    pub responses: Vec<Response>,
    /// The `sync-token` a sync-collection reply ends with.
    pub sync_token: Option<String>,
}

/// Why a body was not a multistatus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DavFault {
    /// Not XML, or not a `multistatus`.
    #[error("not a multistatus")]
    Unreadable,
}

/// Reads a multistatus body.
///
/// A document type declaration is refused rather than expanded (roxmltree's default).
pub fn parse_multistatus(xml: &str) -> Result<Multistatus, DavFault> {
    let doc = Document::parse(xml).map_err(|_| DavFault::Unreadable)?;
    let root = doc.root_element();
    if !root.has_tag_name((DAV, "multistatus")) {
        return Err(DavFault::Unreadable);
    }
    Ok(Multistatus {
        responses: children(root, "response").map(response).collect(),
        sync_token: children(root, "sync-token").next().map(text),
    })
}

impl Response {
    /// The first property called `name` (qualified).
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }

    /// The value of `name` when the server found it.
    pub fn found(&self, name: &str) -> Option<&str> {
        self.prop(name)
            .filter(|p| p.status == PropStatus::Found)
            .map(|p| p.value.as_str())
    }
}

fn response(node: Node<'_, '_>) -> Response {
    let mut props = Vec::new();
    for propstat in children(node, "propstat") {
        let status = children(propstat, "status")
            .next()
            .and_then(|s| code(&text(s)))
            .map_or(PropStatus::Found, prop_status);
        for prop in children(propstat, "prop") {
            props.extend(prop.children().filter(Node::is_element).map(|p| Prop {
                name: qualify(p.tag_name().namespace(), p.tag_name().name()),
                value: value(p),
                status,
            }));
        }
    }
    Response {
        href: children(node, "href").next().map(text).unwrap_or_default(),
        status: children(node, "status").next().and_then(|s| code(&text(s))),
        props,
    }
}

fn prop_status(code: u16) -> PropStatus {
    match code {
        200..=299 => PropStatus::Found,
        404 => PropStatus::Missing,
        other => PropStatus::Other(other),
    }
}

/// A property's value: its text, or when it holds elements, one entry per element joined by
/// spaces (an `href` child contributes its text, any other child its qualified name, so a
/// `resourcetype` reads `DAV:collection urn:ietf:params:xml:ns:carddav:addressbook`).
fn value(node: Node<'_, '_>) -> String {
    let elements: Vec<String> = node
        .children()
        .filter(Node::is_element)
        .map(|c| match c.has_tag_name((DAV, "href")) {
            true => text(c),
            false => qualify(c.tag_name().namespace(), c.tag_name().name()),
        })
        .collect();
    match elements.is_empty() {
        true => text(node),
        false => elements.join(" "),
    }
}

/// `HTTP/1.1 404 Not Found` is 404.
fn code(line: &str) -> Option<u16> {
    line.split_whitespace().nth(1)?.parse().ok()
}

fn children<'a, 'input: 'a>(
    node: Node<'a, 'input>,
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'input>> + 'a {
    node.children()
        .filter(move |c| c.is_element() && c.has_tag_name((DAV, name)))
}

/// Every piece of text under `node`, joined (CDATA included) and trimmed.
fn text(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter(Node::is_text)
        .filter_map(|n| n.text())
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_are_matched_by_namespace_whatever_the_prefix() {
        const CASES: &[&str] = &[
            r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/a</d:href></d:response></d:multistatus>"#,
            r#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/a</D:href></D:response></D:multistatus>"#,
            r#"<multistatus xmlns="DAV:"><response><href>/a</href></response></multistatus>"#,
        ];
        for xml in CASES {
            let reply = parse_multistatus(xml).unwrap();
            assert_eq!(reply.responses.len(), 1, "{xml}");
            assert_eq!(reply.responses[0].href, "/a", "{xml}");
        }
    }

    #[test]
    fn an_element_in_another_namespace_is_not_the_dav_one() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:x="urn:x"><x:response><d:href>/a</d:href></x:response></d:multistatus>"#;
        assert!(parse_multistatus(xml).unwrap().responses.is_empty());
    }

    #[test]
    fn a_failed_propstat_is_marked_not_dropped() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav">
          <d:response><d:href>/p/</d:href>
            <d:propstat><d:prop><d:displayname>Me</d:displayname>
              <d:resourcetype><d:collection/><c:addressbook/><x:other xmlns:x="urn:x"/></d:resourcetype>
            </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
            <d:propstat><d:prop><d:getetag>"lie"</d:getetag></d:prop>
              <d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
            <d:propstat><d:prop><d:sync-token/></d:prop>
              <d:status>HTTP/1.1 403 Forbidden</d:status></d:propstat>
          </d:response></d:multistatus>"#;
        let r = &parse_multistatus(xml).unwrap().responses[0];
        assert_eq!(r.found("DAV:displayname"), Some("Me"));
        assert_eq!(r.found("DAV:getetag"), None);
        assert_eq!(r.prop("DAV:getetag").unwrap().status, PropStatus::Missing);
        assert_eq!(
            r.prop("DAV:sync-token").unwrap().status,
            PropStatus::Other(403)
        );
        assert_eq!(
            r.found("DAV:resourcetype"),
            Some("DAV:collection urn:ietf:params:xml:ns:carddav:addressbook urn:x:other")
        );
    }

    #[test]
    fn a_sync_report_carries_its_new_token_and_its_removals() {
        let xml = r#"<d:multistatus xmlns:d="DAV:">
          <d:response><d:href>/book/a.vcf</d:href>
            <d:propstat><d:prop><d:getetag>"1"</d:getetag></d:prop>
            <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
          <d:response><d:href>/book/b.vcf</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>
          <d:sync-token>http://x.test/sync/2</d:sync-token>
        </d:multistatus>"#;
        let reply = parse_multistatus(xml).unwrap();
        assert_eq!(reply.sync_token.as_deref(), Some("http://x.test/sync/2"));
        let rs = &reply.responses;
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0].found("DAV:getetag"), Some("\"1\""));
        assert_eq!(rs[0].status, None);
        assert_eq!(rs[1].status, Some(404));
    }

    #[test]
    fn principal_and_home_hrefs_are_read_from_inside_their_properties() {
        let xml = r#"<multistatus xmlns="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><response><href>/</href>
          <propstat><prop>
            <current-user-principal><href>/principals/me/</href></current-user-principal>
            <c:addressbook-home-set><href>/home/one/</href><href>/home/two/</href></c:addressbook-home-set>
          </prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
        let r = &parse_multistatus(xml).unwrap().responses[0];
        assert_eq!(
            r.found("DAV:current-user-principal"),
            Some("/principals/me/")
        );
        assert_eq!(
            r.found("urn:ietf:params:xml:ns:carddav:addressbook-home-set"),
            Some("/home/one/ /home/two/")
        );
    }

    #[test]
    fn a_value_in_cdata_is_read_as_text() {
        let xml = "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:carddav\"><d:response><d:href>/a</d:href><d:propstat><d:prop><c:address-data><![CDATA[BEGIN:VCARD\r\nFN:A & B\r\nEND:VCARD]]></c:address-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";
        let data = parse_multistatus(xml).unwrap().responses[0]
            .found("urn:ietf:params:xml:ns:carddav:address-data")
            .unwrap()
            .to_owned();
        assert!(data.contains("FN:A & B"), "{data}");
    }

    #[test]
    fn a_reply_that_is_not_a_multistatus_is_an_error_not_an_empty_one() {
        const CASES: &[&str] = &[
            "<html><body>Sign in</body></html>",
            "not xml at all",
            r#"<!DOCTYPE x [<!ENTITY a "aaaa">]><d:multistatus xmlns:d="DAV:">&a;</d:multistatus>"#,
        ];
        for xml in CASES {
            assert_eq!(parse_multistatus(xml), Err(DavFault::Unreadable), "{xml}");
        }
    }
}
