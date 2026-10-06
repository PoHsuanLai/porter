//! What a loopback redirect may be, from mailo's rules: the listener is on 127.0.0.1 only, reads
//! at most 8 KiB, waits at most 300 s, accepts one request, and the `state` must be the one the
//! sign-in sent. Ported from mailo's loopback listener (`~/mailo/crates/mail-runtime/src/loopback.rs`),
//! with one change: mailo keeps listening after a request with the wrong state; here `wait`
//! takes exactly one request and is gone, so a second one is refused by the OS and a
//! wrong-state redirect ends the sign-in. A host that wants mailo's behaviour opts in with
//! `wait_until`, which is bounded (`MAX_STRAY_REQUESTS`, a deadline).

use crate::form;
use crate::pkce::OAuthState;
use porter_core::SecretText;

/// The most bytes of a redirect request that are read.
pub const MAX_REDIRECT_BYTES: usize = 8 * 1024;
/// How long the listener waits for the person, in seconds.
pub const REDIRECT_WAIT_SECONDS: u64 = 300;
/// The most requests that are not the answer (wrong `state`, probes, malformed) that
/// `LoopbackServer::wait_until` tolerates before it ends.
pub const MAX_STRAY_REQUESTS: usize = 8;
/// How long `wait_until` lets one connection take to send its request line and headers.
pub const STRAY_READ_SECONDS: u64 = 5;

/// The authorization code a redirect carried; secret until exchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCode(pub SecretText);

/// Why a redirect was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoopbackFault {
    /// The `state` is not this sign-in's.
    #[error("wrong state")]
    WrongState,
    /// The request was longer than the cap.
    #[error("redirect too large")]
    Oversized,
    /// The request carried no code.
    #[error("no code")]
    NoCode,
    /// The issuer redirected with an `error`.
    #[error("the issuer refused: {0}")]
    Refused(String),
    /// Not an HTTP request line.
    #[error("not a redirect request")]
    Malformed,
    /// Nobody came within [`REDIRECT_WAIT_SECONDS`].
    #[error("timed out waiting for the browser")]
    TimedOut,
}

/// The code in the redirect request `head` (through the blank line), if its `state` is
/// `expected`.
pub fn parse_redirect(head: &[u8], expected: &OAuthState) -> Result<AuthCode, LoopbackFault> {
    if head.len() > MAX_REDIRECT_BYTES {
        return Err(LoopbackFault::Oversized);
    }
    let text = String::from_utf8_lossy(head);
    let line = text.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let target = match (parts.next(), parts.next(), parts.next()) {
        (Some("GET"), Some(target), Some(version)) if version.starts_with("HTTP/") => target,
        _ => return Err(LoopbackFault::Malformed),
    };
    let params = target
        .split_once('?')
        .map_or(Vec::new(), |(_, q)| form::pairs(q));
    let param = |name: &str| {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    // The state is checked before anything else is believed, an issuer's `error` included: a
    // refusal nobody asked for is as foreign as a code nobody asked for.
    if !param("state").is_some_and(|s| expected.accepts(s)) {
        return Err(LoopbackFault::WrongState);
    }
    match (param("error"), param("code")) {
        (Some(error), _) => Err(LoopbackFault::Refused(error.to_owned())),
        (None, Some(code)) if !code.is_empty() => Ok(AuthCode(SecretText::new(code))),
        _ => Err(LoopbackFault::NoCode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> OAuthState {
        OAuthState("st-1".into())
    }

    fn parse(request: &str) -> Result<String, LoopbackFault> {
        parse_redirect(request.as_bytes(), &state()).map(|c| c.0.expose().to_owned())
    }

    #[test]
    fn a_redirect_parses_to_its_code_or_its_fault() {
        let cases: &[(&str, &str, Result<&str, LoopbackFault>)] = &[
            (
                "code",
                "GET /?code=4%2F0Aa&state=st-1 HTTP/1.1\r\n\r\n",
                Ok("4/0Aa"),
            ),
            (
                "path before query",
                "GET /cb?state=st-1&code=c HTTP/1.1\r\nHost: x\r\n\r\n",
                Ok("c"),
            ),
            (
                "wrong state",
                "GET /?code=stolen&state=wrong HTTP/1.1\r\n\r\n",
                Err(LoopbackFault::WrongState),
            ),
            (
                "no state",
                "GET /?code=stolen HTTP/1.1\r\n\r\n",
                Err(LoopbackFault::WrongState),
            ),
            (
                "foreign error",
                "GET /?error=access_denied&state=x HTTP/1.1\r\n\r\n",
                Err(LoopbackFault::WrongState),
            ),
            (
                "issuer error",
                "GET /?error=access_denied&state=st-1 HTTP/1.1\r\n\r\n",
                Err(LoopbackFault::Refused("access_denied".into())),
            ),
            (
                "no code",
                "GET /?state=st-1 HTTP/1.1\r\n\r\n",
                Err(LoopbackFault::NoCode),
            ),
            (
                "empty code",
                "GET /?code=&state=st-1 HTTP/1.1\r\n\r\n",
                Err(LoopbackFault::NoCode),
            ),
            (
                "favicon",
                "GET /favicon.ico HTTP/1.1\r\n\r\n",
                Err(LoopbackFault::WrongState),
            ),
            (
                "post",
                "POST /?code=c&state=st-1 HTTP/1.1\r\n\r\n",
                Err(LoopbackFault::Malformed),
            ),
            ("junk", "\u{0}\u{1}", Err(LoopbackFault::Malformed)),
            ("empty", "", Err(LoopbackFault::Malformed)),
        ];
        for (name, request, want) in cases {
            assert_eq!(parse(request), want.clone().map(str::to_owned), "{name}");
        }
    }

    #[test]
    fn a_request_over_the_cap_is_refused_before_it_is_read() {
        let long = format!(
            "GET /?code={}&state=st-1 HTTP/1.1\r\n\r\n",
            "a".repeat(MAX_REDIRECT_BYTES)
        );
        assert_eq!(parse(&long), Err(LoopbackFault::Oversized));
        let edge = format!("GET /?state=st-1&code={} HTTP/1.1\r\n\r\n", "a".repeat(100));
        assert!(edge.len() < MAX_REDIRECT_BYTES && parse(&edge).is_ok());
    }
}
