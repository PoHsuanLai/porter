//! Reading and rewriting one request head. Pure: bytes and a plan in, bytes and the body's
//! framing out, or why the request is refused.

use crate::fault::RelayFault;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use porter_core::{Origin, RelayAuth, RelayPlan, UrlScheme};

/// How the request's body is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// No body.
    None,
    /// This many bytes.
    Length(u64),
    /// Chunked.
    Chunked,
}

/// A rewritten head and the framing of the body that follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewritten {
    /// What the server is sent: request line and headers, through the blank line.
    pub head: Vec<u8>,
    /// The body that follows.
    pub framing: Framing,
}

const VERSIONS: [&str; 2] = ["HTTP/1.1", "HTTP/1.0"];

fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// `host[:port]` or `[v6][:port]` as written in a `Host` header or a URI authority: the host in
/// lower case and the port if one is named. User info is refused.
fn authority(text: &str) -> Option<(String, Option<u16>)> {
    if text.contains('@') {
        return None;
    }
    let (host, port) = match text.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']')?;
            (
                host,
                if after.is_empty() {
                    ""
                } else {
                    after.strip_prefix(':')?
                },
            )
        }
        None => text.split_once(':').unwrap_or((text, "")),
    };
    let port = match port {
        "" => None,
        digits => Some(digits.parse::<u16>().ok()?),
    };
    (!host.is_empty()).then(|| (host.to_ascii_lowercase(), port))
}

/// Whether `(host, port)` is `origin`, a missing port being `default`.
fn is_origin(parsed: &(String, Option<u16>), default: u16, origin: &Origin) -> bool {
    parsed.0 == origin.host && parsed.1.unwrap_or(default) == origin.port
}

/// The `Host` value for the origin: the port only when it is not the scheme's default.
fn canonical_host(origin: &Origin) -> String {
    let host = match origin.host.contains(':') {
        true => format!("[{}]", origin.host),
        false => origin.host.clone(),
    };
    match origin.port == origin.scheme.default_port() {
        true => host,
        false => format!("{host}:{}", origin.port),
    }
}

/// The request target as the server gets it (origin-form), checked against `origin`.
fn target_for(target: &str, origin: &Origin) -> Result<String, RelayFault> {
    if target.starts_with('/') || target == "*" {
        return Ok(target.to_owned());
    }
    let (scheme, rest) = target.split_once("://").ok_or(RelayFault::Protocol)?;
    let default = match scheme.to_ascii_lowercase().as_str() {
        "http" => UrlScheme::Http.default_port(),
        "https" => UrlScheme::Https.default_port(),
        _ => return Err(RelayFault::ForeignOrigin),
    };
    let (auth, path) = match rest.find(['/', '?']) {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let parsed = authority(auth).ok_or(RelayFault::ForeignOrigin)?;
    match is_origin(&parsed, default, origin) {
        true if path.starts_with('/') => Ok(path.to_owned()),
        true => Ok(format!("/{path}")),
        false => Err(RelayFault::ForeignOrigin),
    }
}

/// The `Authorization` header line the relay adds, or none for a relay that adds no credential.
fn credential(auth: &RelayAuth, login: &str) -> Option<String> {
    match auth {
        RelayAuth::Password(password) => Some(format!(
            "Basic {}",
            STANDARD.encode(format!("{login}:{}", password.expose()))
        )),
        RelayAuth::AccessToken(token) => Some(format!("Bearer {}", token.expose())),
        RelayAuth::Anonymous => None,
    }
}

/// Reads `head` (request line and headers, through the blank line) and gives the head the
/// server is sent: the target and `Host` checked against the plan's origin, the app's
/// credentials dropped and the relay's added, and the framing of the body.
pub fn rewrite(head: &[u8], plan: &RelayPlan) -> Result<Rewritten, RelayFault> {
    let origin = plan.endpoint.url.origin();
    let text = std::str::from_utf8(head).map_err(|_| RelayFault::Protocol)?;
    let body = text.strip_suffix("\r\n\r\n").ok_or(RelayFault::Protocol)?;
    if body.bytes().any(|b| b == 0) || body.replace("\r\n", "").contains(['\r', '\n']) {
        return Err(RelayFault::Protocol);
    }
    let mut lines = body.split("\r\n");
    let request_line = lines.next().ok_or(RelayFault::Protocol)?;
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(RelayFault::Protocol);
    };
    if !is_token(method) || method.eq_ignore_ascii_case("CONNECT") || !VERSIONS.contains(&version) {
        return Err(RelayFault::Protocol);
    }
    let target = target_for(target, &origin)?;

    let mut kept = Vec::new();
    let mut hosts = Vec::new();
    let (mut lengths, mut codings) = (Vec::new(), Vec::new());
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(RelayFault::Protocol)?;
        // Leading whitespace is obsolete line folding, and whitespace before the colon is
        // refused by RFC 9112 5.1: each is a way to disagree with the server about the head.
        if !is_token(name) {
            return Err(RelayFault::Protocol);
        }
        let value = value.trim_matches([' ', '\t']);
        match name.to_ascii_lowercase().as_str() {
            "authorization" | "proxy-authorization" => {}
            "host" => hosts.push(value),
            "upgrade" => return Err(RelayFault::Protocol),
            "content-length" => {
                lengths.push(value);
                kept.push(line);
            }
            "transfer-encoding" => {
                codings.push(value);
                kept.push(line);
            }
            _ => kept.push(line),
        }
    }
    if let Some(host) = hosts.first() {
        let ok = hosts.len() == 1
            && authority(host)
                .is_some_and(|parsed| is_origin(&parsed, origin.scheme.default_port(), &origin));
        match (hosts.len(), ok) {
            (1, true) => {}
            (1, false) => return Err(RelayFault::ForeignOrigin),
            _ => return Err(RelayFault::Protocol),
        }
    }
    let framing = match (lengths.as_slice(), codings.as_slice()) {
        ([], []) => Framing::None,
        ([length], []) => {
            let digits = !length.is_empty() && length.bytes().all(|b| b.is_ascii_digit());
            match digits.then(|| length.parse::<u64>()) {
                Some(Ok(0)) => Framing::None,
                Some(Ok(n)) => Framing::Length(n),
                _ => return Err(RelayFault::Protocol),
            }
        }
        ([], [coding]) if coding.eq_ignore_ascii_case("chunked") => Framing::Chunked,
        // Both, a repeated length or any other coding: the server might read the end of the
        // body somewhere else than the relay does.
        _ => return Err(RelayFault::Protocol),
    };

    let mut out = format!(
        "{method} {target} {version}\r\nHost: {}\r\n",
        canonical_host(&origin)
    );
    for line in kept {
        out.push_str(line);
        out.push_str("\r\n");
    }
    if let Some(value) = credential(&plan.auth, &plan.endpoint.login.0) {
        out.push_str(&format!("Authorization: {value}\r\n"));
    }
    out.push_str("\r\n");
    Ok(Rewritten {
        head: out.into_bytes(),
        framing,
    })
}
