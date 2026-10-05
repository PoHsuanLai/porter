//! `application/x-www-form-urlencoded` and the OAuth error body, by hand: a sign-in exchange is a
//! handful of short pairs, and nothing else in porter needs a URL crate.
//!
//! The percent-decoder is ported from mailo's loopback listener
//! (`~/mailo/crates/mail-runtime/src/loopback.rs`).

use porter_core::EndpointUrl;
use porter_http::{HttpRequest, HttpResponse, Method};

/// Percent-encodes everything but the RFC 3986 unreserved set.
pub(crate) fn encode(text: &str) -> String {
    text.bytes()
        .fold(String::with_capacity(text.len()), |mut out, b| {
            match b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                true => out.push(char::from(b)),
                false => out.push_str(&format!("%{b:02X}")),
            }
            out
        })
}

/// Decodes `%XX` and `+`; a malformed escape is kept literally rather than dropped, so a
/// truncated code fails at the issuer and not silently here.
pub(crate) fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The pairs of a query string or form body, decoded.
pub(crate) fn pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect()
}

/// Pairs as a form body.
pub(crate) fn body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// A form POST to `url`.
pub(crate) fn post(url: &EndpointUrl, pairs: &[(&str, &str)]) -> HttpRequest {
    HttpRequest::new(Method::Post, url.clone())
        .with_header("Content-Type", "application/x-www-form-urlencoded")
        .with_header("Accept", "application/json")
        .with_body(body(pairs))
}

/// The `error` of an OAuth error body, if the body is one.
pub(crate) fn oauth_error(response: &HttpResponse) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_table() {
        const CASES: &[(&str, &str)] = &[
            ("4%2F0Aa", "4/0Aa"),
            ("x%20y", "x y"),
            ("a+b", "a b"),
            ("a%zz", "a%zz"),
            ("trailing%", "trailing%"),
            ("%e2%9c%93", "\u{2713}"),
        ];
        for (raw, want) in CASES {
            assert_eq!(decode(raw), *want, "{raw}");
        }
    }

    #[test]
    fn encoding_round_trips_and_keeps_unreserved() {
        let text = "a b&c=d/é~-._";
        assert_eq!(decode(&encode(text)), text);
        assert_eq!(encode("Az09-._~"), "Az09-._~");
        assert_eq!(body(&[("a", "1 2"), ("b", "&")]), "a=1%202&b=%26");
    }

    #[test]
    fn pairs_skip_what_is_not_a_pair() {
        assert_eq!(
            pairs("code=1&flag&state=x%20y"),
            vec![("code".into(), "1".into()), ("state".into(), "x y".into())]
        );
        assert!(pairs("").is_empty());
    }
}
