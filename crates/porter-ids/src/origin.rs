//! The origin of a URL: scheme, host and port. A [`UrlScheme`] names the schemes porter dials
//! (web, mail, ManageSieve), with each one's default port; an [`Origin`] is what "the same server"
//! means, with the two questions about its host a caller asks: is it this computer, is it a
//! tailnet's.

use std::fmt;

/// The schemes an endpoint URL may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

impl UrlScheme {
    /// The scheme `text` names, if it is one of these.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "http" => Some(UrlScheme::Http),
            "https" => Some(UrlScheme::Https),
            "imap" => Some(UrlScheme::Imap),
            "imaps" => Some(UrlScheme::Imaps),
            "smtp" => Some(UrlScheme::Smtp),
            "smtps" => Some(UrlScheme::Smtps),
            "sieve" => Some(UrlScheme::Sieve),
            "sieves" => Some(UrlScheme::Sieves),
            "pop3" => Some(UrlScheme::Pop3),
            "pop3s" => Some(UrlScheme::Pop3s),
            _ => None,
        }
    }

    /// The scheme's text (`https`).
    pub fn text(self) -> &'static str {
        match self {
            UrlScheme::Http => "http",
            UrlScheme::Https => "https",
            UrlScheme::Imap => "imap",
            UrlScheme::Imaps => "imaps",
            UrlScheme::Smtp => "smtp",
            UrlScheme::Smtps => "smtps",
            UrlScheme::Sieve => "sieve",
            UrlScheme::Sieves => "sieves",
            UrlScheme::Pop3 => "pop3",
            UrlScheme::Pop3s => "pop3s",
        }
    }

    /// The port used when the URL names none.
    pub fn default_port(self) -> u16 {
        match self {
            UrlScheme::Http => 80,
            UrlScheme::Https => 443,
            UrlScheme::Imap => 143,
            UrlScheme::Imaps => 993,
            UrlScheme::Smtp => 587,
            UrlScheme::Smtps => 465,
            UrlScheme::Sieve | UrlScheme::Sieves => 4190,
            UrlScheme::Pop3 => 110,
            UrlScheme::Pop3s => 995,
        }
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
