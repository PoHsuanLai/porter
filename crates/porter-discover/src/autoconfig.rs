//! Mozilla-style autoconfig: `https://autoconfig.<domain>/mail/config-v1.1.xml` and
//! `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`, then Thunderbird's database.
//!
//! Ported from mailo's `crates/mail-proto/src/discover/mod.rs` (`from_autoconfig`, `pick`,
//! `username`, `expand`; MIT OR Apache-2.0, same author) and the URL list of
//! `crates/mail-runtime/src/discover.rs`. Left in mailo: POP3, the OAuth issuer to preset
//! mapping, the personal-Microsoft guard.
//!
//! Two rules hold whatever a document says, because a document is a stranger's:
//!
//! - **Never plain.** A server offered with no TLS is skipped. Implicit TLS is preferred; a
//!   `STARTTLS` server is used only when no implicit one will do, and its endpoint says
//!   `Tls::StartTls` so the relay refuses to send a credential before the upgrade (mailo skipped
//!   `STARTTLS` entirely; the endpoint type lets porter record it instead).
//! - **Nothing here is a decision to connect.** The [`Found`] goes to the review step.

use crate::config::{self, AuthMethod, ClientConfig, Server, ServerProtocol, SocketType};
use crate::found::{DiscoverFault, Found, Source};
use crate::mail::{imap_claim, imap_endpoint, smtp_endpoint};
use porter_core::{LoginName, ServiceEndpoint, Tls, WebUrl};
use porter_provider::DomainName;

/// Where the public ISPDB is served; the domain is appended.
pub const ISPDB: &str = "https://autoconfig.thunderbird.net/v1.1/";

/// The URLs to try for an address at `domain`, in order: the domain's own `autoconfig` host, its
/// `.well-known` path, then the ISP database. All HTTPS. The first two carry the address as
/// mailo sends it, `?emailaddress=<address>` percent-encoded, because some hosts answer per
/// mailbox; the ISP database is a third party's public service and is asked for the domain only.
pub fn autoconfig_urls(domain: &DomainName, address: &str) -> Vec<WebUrl> {
    let query = format!("?emailaddress={}", percent_encode(address));
    [
        format!("https://autoconfig.{domain}/mail/config-v1.1.xml{query}"),
        format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml{query}"),
        ispdb_url(domain),
    ]
    .iter()
    .filter_map(|text| WebUrl::parse(text).ok())
    .collect()
}

/// `text` as one query value: unreserved characters stay, every other byte is `%XX`.
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The ISP database's document URL for `domain` as text.
pub(crate) fn ispdb_url(domain: &DomainName) -> String {
    format!("{ISPDB}{domain}")
}

/// The servers a `config-v1.1.xml` document names for `address`: incoming IMAP (POP3 is
/// mailo's), outgoing SMTP, each with its security and login pattern resolved.
pub fn parse_autoconfig(xml: &str, address: &str) -> Result<Found, DiscoverFault> {
    let config = config::parse(xml).map_err(|_| DiscoverFault::Unreadable)?;
    select(&config, address)
}

/// The domain of an address: after the last `@`; `None` for something that is not an address.
pub(crate) fn domain_of(address: &str) -> Option<DomainName> {
    let (local, domain) = address.rsplit_once('@')?;
    match local.is_empty() {
        true => None,
        false => DomainName::parse(domain).ok(),
    }
}

fn select(config: &ClientConfig, address: &str) -> Result<Found, DiscoverFault> {
    let incoming = pick(
        &config.incoming,
        &ServerProtocol::Imap,
        address,
        imap_endpoint,
    );
    let outgoing = pick(
        &config.outgoing,
        &ServerProtocol::Smtp,
        address,
        smtp_endpoint,
    );
    match (incoming, outgoing) {
        (Some(imap), Some(smtp)) => Ok(Found {
            endpoints: vec![imap, smtp],
            claims: vec![imap_claim()],
            source: Source::Autoconfig,
        }),
        _ => Err(DiscoverFault::NoServers),
    }
}

/// The best server of `protocol`: the first implicit-TLS one that takes a password and builds a
/// coherent endpoint, else the first `STARTTLS` one.
fn pick(
    servers: &[Server],
    protocol: &ServerProtocol,
    address: &str,
    build: fn(String, u16, Tls, LoginName) -> Option<ServiceEndpoint>,
) -> Option<ServiceEndpoint> {
    let usable = |wanted: &SocketType, tls: Tls| {
        servers
            .iter()
            .filter(|s| &s.protocol == protocol && &s.socket == wanted && takes_password(s))
            .find_map(|s| {
                let host = expand(&s.hostname, address);
                build(host, s.port, tls, login(&s.username, address))
            })
    };
    usable(&SocketType::Ssl, Tls::Implicit).or_else(|| usable(&SocketType::StartTls, Tls::StartTls))
}

/// No `authentication` element means the format's default, a password.
fn takes_password(server: &Server) -> bool {
    server.auth.is_empty() || server.auth.contains(&AuthMethod::PasswordCleartext)
}

/// A `username` element as the login name: an empty element is the whole address, which is what
/// a login almost always is; the format's placeholders are filled in.
fn login(raw: &str, address: &str) -> LoginName {
    let raw = raw.trim();
    match raw.is_empty() {
        true => LoginName(address.to_owned()),
        false => LoginName(expand(raw, address)),
    }
}

/// Substitute the format's address placeholders.
fn expand(raw: &str, address: &str) -> String {
    let (local, domain) = address.rsplit_once('@').unwrap_or((address, ""));
    raw.replace("%EMAILADDRESS%", address)
        .replace("%EMAILLOCALPART%", local)
        .replace("%EMAILDOMAIN%", domain)
}

#[cfg(test)]
mod tests;
