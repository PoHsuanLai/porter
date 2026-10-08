//! The origins a service's pre-authenticated links may point at (design/31 §6.1, W6c): Microsoft
//! Graph answers an upload session with an `uploadUrl`, and a content request with a redirect to
//! a `downloadUrl`, each on another host and each carrying its own authorisation in the URL. An
//! app reaches such a host through `OpenLinked`, which adds no credential; the provider file
//! names which hosts that may be, so the daemon is never an open proxy.

use porter_core::{CoreError, Origin};
use serde::{Deserialize, Serialize};
use std::fmt;

use super::DomainName;

/// One allowed origin, as the file writes it: `host`, `*.suffix` (any host under the suffix, not
/// the suffix itself) or either with `:port`. Without a port only the scheme's default is allowed
/// (443 for `https`), so a port is always a deliberate line in the file. A wildcard names at
/// least two labels after the `*.` (`*.com` is refused).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LinkedOrigin {
    host: LinkedHost,
    port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LinkedHost {
    Exactly(DomainName),
    Under(DomainName),
}

impl LinkedOrigin {
    /// The pattern written as `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let bad = || CoreError::MalformedId {
            what: "linked origin",
            text: text.to_owned(),
        };
        let (host, port) = match text.rsplit_once(':') {
            Some((host, port)) => (
                host,
                Some(
                    port.parse::<u16>()
                        .ok()
                        .filter(|p| *p != 0)
                        .ok_or_else(bad)?,
                ),
            ),
            None => (text, None),
        };
        let host = match host.strip_prefix("*.") {
            Some(suffix) => {
                let name = DomainName::parse(suffix).map_err(|_| bad())?;
                if !name.as_str().contains('.') {
                    return Err(bad());
                }
                LinkedHost::Under(name)
            }
            None => LinkedHost::Exactly(DomainName::parse(host).map_err(|_| bad())?),
        };
        Ok(Self { host, port })
    }

    /// Whether `origin` is one this pattern names: the host, and the port (the scheme's default
    /// when the pattern has none). The scheme is the caller's to check.
    pub fn allows(&self, origin: &Origin) -> bool {
        let port = self.port.unwrap_or(origin.scheme.default_port());
        let host = match DomainName::parse(&origin.host) {
            Ok(host) => host,
            Err(_) => return false,
        };
        let host_ok = match &self.host {
            LinkedHost::Exactly(name) => host == *name,
            LinkedHost::Under(suffix) => host != *suffix && host.is_within(suffix),
        };
        host_ok && origin.port == port
    }
}

impl fmt::Display for LinkedOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            LinkedHost::Exactly(name) => write!(f, "{}", name.as_str())?,
            LinkedHost::Under(suffix) => write!(f, "*.{}", suffix.as_str())?,
        }
        match self.port {
            Some(port) => write!(f, ":{port}"),
            None => Ok(()),
        }
    }
}

impl TryFrom<String> for LinkedOrigin {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<LinkedOrigin> for String {
    fn from(origin: LinkedOrigin) -> String {
        origin.to_string()
    }
}

/// One origin a service hands out links to that DO need the account's bearer (Google Photos'
/// picked items on `lh3.googleusercontent.com`), as the file writes it: an exact `host` or
/// `host:port`. Never a wildcard: the daemon adds the account's credential to what it dials, so
/// every host is named. The scheme is the home endpoint's (a relay never goes to a less secure
/// scheme than the service it belongs to; for a real service that is `https`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AuthOrigin(LinkedOrigin);

impl AuthOrigin {
    /// The origin written as `text`, or why it is not one (a wildcard is not).
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        if text.contains('*') {
            return Err(CoreError::MalformedId {
                what: "authenticated origin",
                text: text.to_owned(),
            });
        }
        LinkedOrigin::parse(text).map(Self)
    }

    /// Whether `origin` is exactly this one (the scheme is the caller's to check).
    pub fn allows(&self, origin: &Origin) -> bool {
        self.0.allows(origin)
    }
}

impl fmt::Display for AuthOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<String> for AuthOrigin {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<AuthOrigin> for String {
    fn from(origin: AuthOrigin) -> String {
        origin.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::EndpointUrl;

    fn origin(url: &str) -> Origin {
        EndpointUrl::parse(url).expect("url").origin()
    }

    #[test]
    fn a_pattern_names_hosts_by_label_and_the_default_port_unless_it_says_one() {
        const CASES: &[(&str, &str, bool)] = &[
            ("*.up.1drv.com", "https://sn3302.up.1drv.com", true),
            ("*.up.1drv.com", "https://a.b.up.1drv.com", true),
            ("*.up.1drv.com", "https://up.1drv.com", false),
            ("*.up.1drv.com", "https://evilup.1drv.com", false),
            ("*.up.1drv.com", "https://x.up.1drv.com.evil.example", false),
            ("*.up.1drv.com", "https://x.up.1drv.com:8443", false),
            ("files.example.org", "https://files.example.org", true),
            ("files.example.org", "https://a.files.example.org", false),
            ("127.0.0.1:9000", "http://127.0.0.1:9000", true),
            ("127.0.0.1:9000", "http://127.0.0.1:9001", false),
            ("127.0.0.1:9000", "http://127.0.0.1", false),
        ];
        for (pattern, url, want) in CASES {
            let pattern = LinkedOrigin::parse(pattern).expect("pattern");
            assert_eq!(pattern.allows(&origin(url)), *want, "{pattern} vs {url}");
        }
    }

    #[test]
    fn an_authenticated_origin_is_one_exact_host_and_never_a_wildcard() {
        let lh3 = AuthOrigin::parse("lh3.googleusercontent.com").expect("origin");
        assert!(lh3.allows(&origin("https://lh3.googleusercontent.com")));
        assert!(!lh3.allows(&origin("https://lh4.googleusercontent.com")));
        assert!(!lh3.allows(&origin("https://a.lh3.googleusercontent.com")));
        assert!(!lh3.allows(&origin("https://lh3.googleusercontent.com:8443")));
        for text in ["*.googleusercontent.com", "*", "lh*.example.org", "", "a b"] {
            assert!(AuthOrigin::parse(text).is_err(), "{text:?}");
        }
        assert_eq!(lh3.to_string(), "lh3.googleusercontent.com");
    }

    #[test]
    fn patterns_that_would_name_too_much_are_refused() {
        for text in [
            "*",
            "*.com",
            "*.",
            "",
            "*.up.1drv.com:0",
            "a b.example",
            "x:y",
            "a..b",
        ] {
            assert!(LinkedOrigin::parse(text).is_err(), "{text:?}");
        }
        for text in [
            "*.up.1drv.com",
            "files.example.org:8443",
            "*.sharepoint.com",
        ] {
            assert_eq!(LinkedOrigin::parse(text).expect(text).to_string(), text);
        }
    }
}
