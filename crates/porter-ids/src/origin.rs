//! The origin of a URL: scheme, host and port. A [`UrlScheme`] names the schemes porter dials
//! (web, mail, ManageSieve), with each one's default port; an [`Origin`] is what "the same server"
//! means, with the two questions about its host a caller asks: is it this computer, is it a
//! tailnet's.

use std::fmt;

/// The schemes an endpoint URL may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum UrlScheme {
    /// `http`.
    Http,
    /// `https`.
    Https,
    /// `imap`.
    Imap,
    /// `imaps`.
    Imaps,
    /// `smtp`.
    Smtp,
    /// `smtps`.
    Smtps,
    /// `sieve` (ManageSieve, STARTTLS).
    Sieve,
    /// `sieves` (ManageSieve over TLS from the first byte).
    Sieves,
    /// `pop3` (POP3, STARTTLS as `STLS`).
    Pop3,
    /// `pop3s` (POP3 over TLS from the first byte).
    Pop3s,
}

/// Each scheme with its text and the port used when the URL names none: the one table `parse`,
/// `text` and `default_port` read.
const SCHEMES: [(UrlScheme, &str, u16); 10] = [
    (UrlScheme::Http, "http", 80),
    (UrlScheme::Https, "https", 443),
    (UrlScheme::Imap, "imap", 143),
    (UrlScheme::Imaps, "imaps", 993),
    (UrlScheme::Smtp, "smtp", 587),
    (UrlScheme::Smtps, "smtps", 465),
    (UrlScheme::Sieve, "sieve", 4190),
    (UrlScheme::Sieves, "sieves", 4190),
    (UrlScheme::Pop3, "pop3", 110),
    (UrlScheme::Pop3s, "pop3s", 995),
];

impl UrlScheme {
    /// The scheme `text` names, if it is one of these.
    pub fn parse(text: &str) -> Option<Self> {
        SCHEMES
            .iter()
            .find(|(_, name, _)| *name == text)
            .map(|(scheme, ..)| *scheme)
    }

    fn row(self) -> &'static (UrlScheme, &'static str, u16) {
        SCHEMES
            .iter()
            .find(|(scheme, ..)| *scheme == self)
            .unwrap_or_else(|| unreachable!("every scheme has a row in SCHEMES"))
    }

    /// The scheme's text (`https`).
    pub fn text(self) -> &'static str {
        self.row().1
    }

    /// The port used when the URL names none.
    pub fn default_port(self) -> u16 {
        self.row().2
    }
}

/// Scheme, host and port: what "the same server" means.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    /// The scheme.
    pub scheme: UrlScheme,
    /// The host in lower case, an IPv6 literal without its brackets.
    pub host: String,
    /// The port, the scheme's default when the URL named none.
    pub port: u16,
}

/// `host[:port]` or `[v6][:port]`; the host is never empty.
pub fn split_authority(authority: &str) -> Option<(&str, Option<&str>)> {
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']')?;
            (host, after.strip_prefix(':'))
        }
        None => match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    let host_ok = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'));
    host_ok.then_some((host, port))
}

impl Origin {
    /// Whether the host is this computer (`localhost`, `127.0.0.0/8`, `::1`).
    pub fn is_loopback(&self) -> bool {
        self.host == "localhost"
            || self
                .host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    }

    /// Whether the host is an address only a tailnet hands out: one in Tailscale's ranges
    /// (100.64.0.0/10, fd7a:115c:a1e0::/48), a MagicDNS name under `.ts.net`, or a MagicDNS
    /// short name (one label, no dot), which only a tailnet's resolver answers for.
    pub fn is_tailnet(&self) -> bool {
        match self.host.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(ip)) => {
                let [a, b, ..] = ip.octets();
                a == 100 && (64..128).contains(&b)
            }
            Ok(std::net::IpAddr::V6(ip)) => {
                let s = ip.segments();
                s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
            }
            Err(_) => {
                let host = self.host.trim_end_matches('.');
                host.ends_with(".ts.net")
                    || (!host.is_empty() && !host.contains('.') && host != "localhost")
            }
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}:{}", self.scheme.text(), self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names every variant, so a new scheme fails to compile here until it has a row.
    fn is_a_scheme(scheme: UrlScheme) -> bool {
        match scheme {
            UrlScheme::Http
            | UrlScheme::Https
            | UrlScheme::Imap
            | UrlScheme::Imaps
            | UrlScheme::Smtp
            | UrlScheme::Smtps
            | UrlScheme::Sieve
            | UrlScheme::Sieves
            | UrlScheme::Pop3
            | UrlScheme::Pop3s => true,
        }
    }

    #[test]
    fn every_scheme_has_its_text_and_port_and_reads_back() {
        for (scheme, text, port) in SCHEMES {
            assert!(is_a_scheme(scheme));
            assert_eq!(scheme.text(), text);
            assert_eq!(scheme.default_port(), port);
            assert_eq!(UrlScheme::parse(text), Some(scheme));
        }
        assert_eq!(SCHEMES.len(), 10);
        assert_eq!(UrlScheme::parse("ftp"), None);
        assert_eq!(UrlScheme::parse("HTTPS"), None);
    }
}
