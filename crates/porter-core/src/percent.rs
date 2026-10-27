//! Percent-encoding, once: the character sets porter encodes with, and the one decoder.
//!
//! The work is `percent-encoding`'s; this module names the sets so every crate spells a path
//! segment, a path, and a query value the same way. A `+` is a space only in a form body or
//! query, which `form_urlencoded` reads; a path never reads it that way, so [`decode`] keeps it.

pub use percent_encoding::AsciiSet;
use percent_encoding::{percent_decode_str, utf8_percent_encode};

/// Everything but RFC 3986 unreserved characters (`A-Z a-z 0-9 - . _ ~`) is escaped. This is the
/// set of a query value and of one path segment.
pub const UNRESERVED: &AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// A whole path: unreserved characters and the `/` between its segments stay.
pub const PATH: &AsciiSet = &UNRESERVED.remove(b'/');

/// `text` with every character outside `keep` escaped as `%XX` (upper-case hex).
#[must_use]
pub fn encode(text: &str, keep: &'static AsciiSet) -> String {
    utf8_percent_encode(text, keep).to_string()
}

/// `text` as one path segment or one query value.
#[must_use]
pub fn encode_unreserved(text: &str) -> String {
    encode(text, UNRESERVED)
}

/// `text` as a path: its `/` stay.
#[must_use]
pub fn encode_path(text: &str) -> String {
    encode(text, PATH)
}

/// `%XX` escapes decoded to bytes. A `%` not followed by two hex digits is kept as written, and
/// `+` stays a `+`.
#[must_use]
pub fn decode_bytes(text: &str) -> Vec<u8> {
    percent_decode_str(text).collect()
}

/// [`decode_bytes`] as text; a byte sequence that is not UTF-8 becomes U+FFFD.
#[must_use]
pub fn decode(text: &str) -> String {
    percent_decode_str(text).decode_utf8_lossy().into_owned()
}

/// Whether every `%` of `text` is followed by two hex digits. The decoders keep a bad escape as
/// written; a caller that must refuse one (a name a server could read another way) asks first.
#[must_use]
pub fn is_well_formed(text: &str) -> bool {
    text.split('%').skip(1).all(|rest| {
        rest.as_bytes()
            .get(..2)
            .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
    })
}

/// [`decode`] as text, or `None` for a malformed escape or bytes that are not UTF-8.
#[must_use]
pub fn decode_strict(text: &str) -> Option<String> {
    if !is_well_formed(text) {
        return None;
    }
    percent_decode_str(text)
        .decode_utf8()
        .ok()
        .map(std::borrow::Cow::into_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreserved_stay_and_the_rest_is_upper_case_hex() {
        assert_eq!(encode_unreserved("Az09-._~"), "Az09-._~");
        assert_eq!(
            encode_unreserved("a b&c=d/\u{e9}"),
            "a%20b%26c%3Dd%2F%C3%A9"
        );
        assert_eq!(encode_unreserved("*+"), "%2A%2B");
    }

    #[test]
    fn a_path_keeps_its_slashes() {
        assert_eq!(encode_path("/a b/c%d"), "/a%20b/c%25d");
    }

    #[test]
    fn decoding_keeps_a_bad_escape_and_a_plus() {
        const CASES: &[(&str, &str)] = &[
            ("4%2F0Aa", "4/0Aa"),
            ("x%20y", "x y"),
            ("a+b", "a+b"),
            ("a%zz", "a%zz"),
            ("trailing%", "trailing%"),
            ("trailing%4", "trailing%4"),
            ("%e2%9c%93", "\u{2713}"),
        ];
        for (raw, want) in CASES {
            assert_eq!(decode(raw), *want, "{raw}");
        }
    }

    #[test]
    fn bytes_and_strict_text() {
        assert_eq!(decode_bytes("a%00%ff"), b"a\0\xff");
        assert_eq!(decode_strict("ada%40x.org").as_deref(), Some("ada@x.org"));
        assert_eq!(decode_strict("%ff"), None);
        assert_eq!(decode_strict("a%zz"), None);
        assert!(is_well_formed("a%2fb%2F") && !is_well_formed("a%2") && !is_well_formed("%"));
        assert_eq!(decode("%ff"), "\u{fffd}");
    }
}
