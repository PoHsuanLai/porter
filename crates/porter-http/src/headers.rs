//! Header names and values. A name is compared without case; a value that can carry a
//! credential never shows in `Debug`.

use std::fmt;

/// A header name, held in lower case.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HeaderName(String);

impl HeaderName {
    /// The name written as `text`, lower-cased.
    pub fn new(text: &str) -> Self {
        Self(text.to_ascii_lowercase())
    }

    /// The name in lower case.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the value under this name can carry a credential.
    fn is_sensitive(&self) -> bool {
        matches!(
            self.0.as_str(),
            "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
        )
    }
}

/// A header value. Alone it does not know which header it is under, so its `Debug` shows
/// nothing; `Header`'s shows it unless the name can carry a credential.
#[derive(Clone, PartialEq, Eq)]
pub struct HeaderValue(pub String);

impl fmt::Debug for HeaderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HeaderValue(..)")
    }
}

/// One header.
#[derive(Clone, PartialEq, Eq)]
pub struct Header {
    /// Its name.
    pub name: HeaderName,
    /// Its value.
    pub value: HeaderValue,
}

impl Header {
    /// The header `name: value`.
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Self {
            name: HeaderName::new(name),
            value: HeaderValue(value.into()),
        }
    }
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shown = match self.name.is_sensitive() {
            true => "<redacted>",
            false => self.value.0.as_str(),
        };
        write!(f, "{}: {shown}", self.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_without_case() {
        assert_eq!(
            HeaderName::new("Content-Type"),
            HeaderName::new("content-type")
        );
    }

    #[test]
    fn a_credential_never_shows_in_debug() {
        const CASES: &[(&str, &str, bool)] = &[
            ("Authorization", "Bearer abc123", false),
            ("Proxy-Authorization", "Basic abc123", false),
            ("Cookie", "sid=abc123", false),
            ("Content-Type", "text/xml", true),
        ];
        for (name, value, shown) in CASES {
            let text = format!("{:?}", Header::new(name, *value));
            assert_eq!(text.contains(value), *shown, "{name}: {text}");
        }
    }
}
