//! Reading a mail client configuration document: the `clientConfig` XML format (version 1.1)
//! that a domain serves at `autoconfig.<domain>` or `/.well-known/autoconfig/`, and that the
//! public ISPDB serves for the domains it knows.
//!
//! Ported from mailo's `crates/mail-proto/src/discover/autoconfig.rs` (MIT OR Apache-2.0, same
//! author). It reads the document into values and decides nothing: which server to use is
//! `autoconfig::select`'s question, so the parser's tests are about the format and the
//! selection's are about the rules.
//!
//! The document is a stranger's. A server entry this cannot make sense of (no hostname, a port
//! that is not a number) is dropped on its own rather than failing the document, because the
//! entry after it may be the good one. A document type declaration is refused outright
//! (roxmltree's default): nothing here needs one, and entity expansion is how an XML answer is
//! made to eat memory.

use roxmltree::{Document, Node};

/// What a configuration document offers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ClientConfig {
    /// Every `incomingServer`, in document order, which is the publisher's order of preference.
    pub(crate) incoming: Vec<Server>,
    /// Every `outgoingServer`, in document order.
    pub(crate) outgoing: Vec<Server>,
    /// The `oAuth2/issuer` host, when the document names an authorization server.
    #[allow(dead_code)]
    // read by the OAuth lane (W5b), which maps an issuer host to a provider
    pub(crate) oauth_issuer: Option<String>,
}

/// One server entry, as written. Placeholders such as `%EMAILADDRESS%` are left in place;
/// substituting them needs the address, which is the selection's input, not the parser's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Server {
    pub(crate) protocol: ServerProtocol,
    pub(crate) hostname: String,
    pub(crate) port: u16,
    pub(crate) socket: SocketType,
    /// The `username` element's text, placeholders and all. Empty when the entry has none.
    pub(crate) username: String,
    /// Every `authentication` element, in order.
    pub(crate) auth: Vec<AuthMethod>,
}

/// The `type` attribute of a server entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServerProtocol {
    Imap,
    Pop3,
    Smtp,
    /// Anything else (`exchange`, `jmap`, …): kept so a caller can say it was offered, never used.
    Other(String),
}

/// The `socketType` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SocketType {
    /// `SSL`: TLS from the first byte. The only one discovery accepts.
    Ssl,
    /// `STARTTLS`: an upgrade after a cleartext greeting.
    StartTls,
    /// `plain`: no TLS at all.
    Plain,
    /// A value the format does not define.
    Other(String),
}

/// One `authentication` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthMethod {
    /// `password-cleartext`: `PLAIN` or `LOGIN`, which over TLS is not cleartext on the wire.
    PasswordCleartext,
    /// `password-encrypted`: a challenge-response mechanism such as `CRAM-MD5`.
    PasswordEncrypted,
    /// `OAuth2`: `XOAUTH2` or `OAUTHBEARER`, against the document's issuer.
    OAuth2,
    /// Anything else (`NTLM`, `GSSAPI`, `client-IP-address`, …).
    Other(String),
}

/// Why a document could not be read at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AutoconfigError {
    /// Not well-formed XML, or it carried a document type declaration.
    #[error("not a well-formed XML document: {0}")]
    Xml(String),
    /// Well-formed, but not a `clientConfig` — an HTML error page served with `200`, say.
    #[error("the document is not a mail client configuration")]
    NotClientConfig,
    /// A `clientConfig` with no `emailProvider` in it.
    #[error("the configuration names no email provider")]
    NoProvider,
}

/// Read a configuration document.
///
/// Only the first `emailProvider` is read. The format allows one per document and every
/// publisher writes one; a second would be a different provider for the same domain, and which
/// of them is meant is not something a client can decide.
pub(crate) fn parse(xml: &str) -> Result<ClientConfig, AutoconfigError> {
    let doc = Document::parse(xml).map_err(|e| AutoconfigError::Xml(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "clientConfig" {
        return Err(AutoconfigError::NotClientConfig);
    }
    let provider = children(root, "emailProvider")
        .next()
        .ok_or(AutoconfigError::NoProvider)?;
    Ok(ClientConfig {
        incoming: children(provider, "incomingServer")
            .filter_map(server)
            .collect(),
        outgoing: children(provider, "outgoingServer")
            .filter_map(server)
            .collect(),
        oauth_issuer: children(root, "oAuth2")
            .next()
            .and_then(|oauth| child_text(oauth, "issuer"))
            .filter(|issuer| !issuer.is_empty()),
    })
}

/// One server entry, or `None` when it names no usable host and port.
fn server(node: Node<'_, '_>) -> Option<Server> {
    let hostname = child_text(node, "hostname").filter(|h| !h.is_empty())?;
    let port = child_text(node, "port")?
        .parse::<u16>()
        .ok()
        .filter(|p| *p != 0)?;
    Some(Server {
        protocol: match node
            .attribute("type")
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("imap") => ServerProtocol::Imap,
            Some("pop3") => ServerProtocol::Pop3,
            Some("smtp") => ServerProtocol::Smtp,
            other => ServerProtocol::Other(other.unwrap_or_default().to_owned()),
        },
        hostname,
        port,
        socket: match child_text(node, "socketType") {
            Some(s) if s.eq_ignore_ascii_case("SSL") => SocketType::Ssl,
            Some(s) if s.eq_ignore_ascii_case("STARTTLS") => SocketType::StartTls,
            Some(s) if s.eq_ignore_ascii_case("plain") => SocketType::Plain,
            // Absent means the format's default, which is no TLS.
            None => SocketType::Plain,
            Some(other) => SocketType::Other(other),
        },
        username: child_text(node, "username").unwrap_or_default(),
        auth: children(node, "authentication")
            .map(|a| {
                let text = text(a);
                match text.to_ascii_lowercase().as_str() {
                    "password-cleartext" | "plain" => AuthMethod::PasswordCleartext,
                    "password-encrypted" | "secure" => AuthMethod::PasswordEncrypted,
                    "oauth2" => AuthMethod::OAuth2,
                    _ => AuthMethod::Other(text),
                }
            })
            .collect(),
    })
}

/// Element children named `name`, whatever their namespace: the format has none, and a
/// publisher that adds a default namespace still means the same elements.
fn children<'a, 'input: 'a>(
    node: Node<'a, 'input>,
    name: &'static str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children()
        .filter(move |n| n.is_element() && n.tag_name().name() == name)
}

fn child_text(node: Node<'_, '_>, name: &'static str) -> Option<String> {
    children(node, name).next().map(text)
}

/// An element's text, trimmed: publishers indent their documents.
fn text(node: Node<'_, '_>) -> String {
    node.text().unwrap_or_default().trim().to_owned()
}

#[cfg(test)]
mod tests;
