//! A `207 Multi-Status` body (RFC 4918 section 13).
//!
//! Elements are matched by namespace and local name, never by prefix: one server writes
//! `<d:href>`, another `<D:href>`, a third declares `DAV:` as the default namespace and writes
//! `<href>`, and all three mean the same element.

use crate::Error;
use crate::names::{DAV, qualify};
use roxmltree::{Document, Node};

/// One property and the status it came with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    /// The property's name, namespace-qualified (`DAV:getetag`).
    pub name: String,
    /// Its text, empty for a property with no value. A property that holds elements reads as
    /// one entry per element joined by spaces: an `href` child gives its text, any other child
    /// its qualified name, so a `resourcetype` reads
    /// `DAV:collection urn:ietf:params:xml:ns:carddav:addressbook`.
    pub value: String,
    /// The status of the property's `propstat`.
    pub status: PropStatus,
}

/// The status of a property group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropStatus {
    /// 2xx, or no status line.
    Found,
    /// 404: the server does not have it.
    Missing,
    /// Another status.
    Other(u16),
}

/// One `response` element: an href and its properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The resource as the server wrote it: usually an absolute path, sometimes a full URL,
    /// possibly percent-encoded. Resolving it against the request's URL is the caller's.
    pub href: String,
    /// The status of the response as a whole, where it has one instead of per-property ones
    /// (in a sync-collection reply, 404 reports a removed member and 507 a cut-short report).
    pub status: Option<u16>,
    /// Its properties, from every `propstat`.
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

/// Reads a multistatus body.
///
/// A document type declaration is refused rather than expanded (roxmltree's default): a server
/// has no reason to send one, and entity expansion is how an XML reply is made to eat memory.
pub fn parse_multistatus(xml: &str) -> Result<Multistatus, Error> {
    let doc = Document::parse(xml).map_err(|e| Error::Xml(e.to_string()))?;
    let root = doc.root_element();
    if !root.has_tag_name((DAV, "multistatus")) {
        return Err(Error::NotMultistatus);
    }
    Ok(Multistatus {
        responses: children(root, "response").map(response).collect(),
        sync_token: children(root, "sync-token").next().map(text),
    })
}

/// The code of an HTTP status line: `HTTP/1.1 404 Not Found` is 404.
pub fn parse_status_line(line: &str) -> Option<u16> {
    line.split_whitespace().nth(1)?.parse().ok()
}

impl Response {
    /// The first property called `name` (qualified), whatever its status.
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }

    /// The value of `name` when the server found it.
    pub fn found(&self, name: &str) -> Option<&str> {
        self.props
            .iter()
            .find(|p| p.name == name && p.status == PropStatus::Found)
            .map(|p| p.value.as_str())
    }

    /// Whether the response reports the resource as no longer there (a 404 status).
    pub fn gone(&self) -> bool {
        self.status == Some(404)
    }
}

fn response(node: Node<'_, '_>) -> Response {
    let mut props = Vec::new();
    for propstat in children(node, "propstat") {
        let status = children(propstat, "status")
            .next()
            .and_then(|s| parse_status_line(&text(s)))
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
        status: children(node, "status")
            .next()
            .and_then(|s| parse_status_line(&text(s))),
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

/// A property's value, as [`Prop::value`] describes it.
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
    fn a_property_found_in_a_later_propstat_is_found() {
        let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/a</d:href>
            <d:propstat><d:prop><d:getetag/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
            <d:propstat><d:prop><d:getetag>"2"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
          </d:response></d:multistatus>"#;
        let r = &parse_multistatus(xml).unwrap().responses[0];
        assert_eq!(r.found("DAV:getetag"), Some("\"2\""));
    }

    #[test]
    fn status_lines() {
        const CASES: &[(&str, Option<u16>)] = &[
            ("HTTP/1.1 404 Not Found", Some(404)),
            ("HTTP/1.1 207 Multi-Status", Some(207)),
            ("  HTTP/1.1   200   OK ", Some(200)),
            ("HTTP/1.1", None),
            ("HTTP/1.1 OK", None),
            ("", None),
        ];
        for (line, want) in CASES {
            assert_eq!(parse_status_line(line), *want, "{line}");
        }
    }

    #[test]
    fn a_reply_that_is_not_a_multistatus_says_why() {
        assert_eq!(
            parse_multistatus("<html><body>Sign in</body></html>"),
            Err(Error::NotMultistatus)
        );
        assert!(matches!(
            parse_multistatus("not xml at all"),
            Err(Error::Xml(_))
        ));
    }
}
