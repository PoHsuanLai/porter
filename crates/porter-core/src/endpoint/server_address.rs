//! Which address the person may give for an AI server of their own (an OpenAI-compatible
//! server: a llama.cpp or vLLM on the same network, a gateway, a company's proxy).
//!
//! The rule, pure and with no environment read:
//!
//! * `https` is accepted for any host;
//! * plain `http` is accepted for this computer (`Origin::is_loopback`) and for one of the
//!   person's own computers on their tailnet. Another person's computer, a tagged server and
//!   every other host need `https`.
//!
//! Which hosts are the person's own computers is an input: the caller reads the list from
//! wherever it has one (accountd's `Tailnet1.Machines`) and passes a question about a host; a
//! caller with no list passes one that says no, and so accepts plain `http` for this computer
//! only. This module knows nothing of what a tailnet's computer is.

use super::{EndpointUrl, Tls, UrlScheme};

/// What a person reads when an address is refused for its scheme.
pub const SERVER_ADDRESS_RULE: &str =
    "Use an https address, or http only for this computer or one of your own computers.";

/// Why an address is not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ServerAddressFault {
    /// Not a web address (`http` or `https`, a host, an optional port and path).
    #[error("not a web address")]
    NotAnAddress,
    /// Plain `http` to a host that is neither this computer nor one of the person's own.
    #[error("plain http is for this computer and your own computers only")]
    PlainOffTrusted,
}

/// An accepted address: where the server is and how the connection is secured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddress {
    url: EndpointUrl,
    tls: Tls,
}

impl ServerAddress {
    /// The address as typed (surrounding blanks removed).
    pub fn url(&self) -> &EndpointUrl {
        &self.url
    }

    /// `Implicit` for `https`, `Plain` for `http`.
    pub fn tls(&self) -> Tls {
        self.tls
    }

    /// The path every request goes under: the address's own path without a closing `/`, empty
    /// for the root (`https://llm.example.test/v1/` is `/v1`).
    pub fn base(&self) -> String {
        self.url.path().trim_end_matches('/').to_owned()
    }
}

/// The address `text` stands for, or why it is refused; `is_own` says whether a host (a name or
/// an address, as it is written in the address) is one of the person's own computers on their
/// tailnet, as far as the caller knows them.
///
/// # Errors
/// [`ServerAddressFault::NotAnAddress`] for text that is not an `http` or `https` address;
/// [`ServerAddressFault::PlainOffTrusted`] for `http` to a host that is not this computer nor one
/// of the person's own computers.
pub fn server_address(
    text: &str,
    is_own: impl Fn(&str) -> bool,
) -> Result<ServerAddress, ServerAddressFault> {
    let url = EndpointUrl::parse(text.trim()).map_err(|_| ServerAddressFault::NotAnAddress)?;
    let origin = url.origin();
    match origin.scheme {
        UrlScheme::Https => Ok(ServerAddress {
            url,
            tls: Tls::Implicit,
        }),
        UrlScheme::Http
            if origin.is_loopback() || (origin.is_tailnet() && is_own(&origin.host)) =>
        {
            Ok(ServerAddress {
                url,
                tls: Tls::Plain,
            })
        }
        UrlScheme::Http => Err(ServerAddressFault::PlainOffTrusted),
        _ => Err(ServerAddressFault::NotAnAddress),
    }
}

/// The address `text` stands for as far as the address alone says: `https`, or `http` to this
/// computer, or `http` to a host that may be one of the person's own computers (which only the
/// list of computers can tell). A form that cannot read the tailnet checks with this and leaves
/// the rest to [`server_address`].
///
/// # Errors
/// [`ServerAddressFault::NotAnAddress`] for text that is not an `http` or `https` address.
pub fn server_address_shape(text: &str) -> Result<EndpointUrl, ServerAddressFault> {
    let url = EndpointUrl::parse(text.trim()).map_err(|_| ServerAddressFault::NotAnAddress)?;
    match url.origin().scheme {
        UrlScheme::Https | UrlScheme::Http => Ok(url),
        _ => Err(ServerAddressFault::NotAnAddress),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The person's own computers, as the hosts they answer to.
    fn is_own(host: &str) -> bool {
        ["studio", "studio.tail1.ts.net", "100.64.0.7"]
            .iter()
            .any(|own| host.eq_ignore_ascii_case(own))
    }

    #[test]
    fn an_address_is_accepted_by_its_scheme_and_its_host() {
        const ACCEPTED: Tls = Tls::Implicit;
        const PLAIN: Tls = Tls::Plain;
        let cases: &[(&str, &str, Result<Tls, ServerAddressFault>)] = &[
            (
                "https, any host",
                "https://llm.example.test/v1",
                Ok(ACCEPTED),
            ),
            (
                "https with a port",
                "https://llm.example.test:8443/v1",
                Ok(ACCEPTED),
            ),
            (
                "blanks around it",
                "  https://llm.example.test/v1  ",
                Ok(ACCEPTED),
            ),
            ("http, localhost", "http://localhost:8080/v1", Ok(PLAIN)),
            ("http, 127.0.0.1", "http://127.0.0.1:8080/v1", Ok(PLAIN)),
            ("http, ::1", "http://[::1]:8080/v1", Ok(PLAIN)),
            (
                "http, my machine by name",
                "http://studio:8080/v1",
                Ok(PLAIN),
            ),
            (
                "http, my machine by name in capitals",
                "http://Studio:8080/v1",
                Ok(PLAIN),
            ),
            (
                "http, my machine by network name",
                "http://studio.tail1.ts.net:8080/v1",
                Ok(PLAIN),
            ),
            (
                "http, my machine by address",
                "http://100.64.0.7:8080/v1",
                Ok(PLAIN),
            ),
            (
                "http, a shared machine",
                "http://friend:8080/v1",
                Err(ServerAddressFault::PlainOffTrusted),
            ),
            (
                "http, a tagged machine",
                "http://gpu-box.tail1.ts.net/v1",
                Err(ServerAddressFault::PlainOffTrusted),
            ),
            (
                "http, an address on the tailnet nobody owns",
                "http://100.64.0.99:8080/v1",
                Err(ServerAddressFault::PlainOffTrusted),
            ),
            (
                "http, any other host",
                "http://llm.example.test/v1",
                Err(ServerAddressFault::PlainOffTrusted),
            ),
            (
                "not a web address",
                "ftp://llm.example.test",
                Err(ServerAddressFault::NotAnAddress),
            ),
            (
                "no scheme",
                "llm.example.test/v1",
                Err(ServerAddressFault::NotAnAddress),
            ),
            (
                "a mail scheme",
                "imaps://mail.example.test",
                Err(ServerAddressFault::NotAnAddress),
            ),
            (
                "a query",
                "https://llm.example.test/v1?key=1",
                Err(ServerAddressFault::NotAnAddress),
            ),
            ("empty", "", Err(ServerAddressFault::NotAnAddress)),
        ];
        for (what, text, want) in cases {
            let got = server_address(text, is_own).map(|address| address.tls());
            assert_eq!(got, *want, "{what}: {text}");
        }
        // With no list of computers only this computer may be plain.
        assert_eq!(
            server_address("http://studio:8080/v1", |_| false),
            Err(ServerAddressFault::PlainOffTrusted)
        );
    }

    #[test]
    fn the_base_is_the_path_without_a_closing_slash() {
        for (text, base) in [
            ("https://llm.example.test", ""),
            ("https://llm.example.test/", ""),
            ("https://llm.example.test/v1", "/v1"),
            ("https://llm.example.test/v1/", "/v1"),
            ("https://llm.example.test/api/v1", "/api/v1"),
        ] {
            let address = server_address(text, |_| false).expect(text);
            assert_eq!(address.base(), base, "{text}");
        }
    }

    #[test]
    fn the_shape_check_leaves_plain_hosts_to_the_list_of_computers() {
        for (text, ok) in [
            ("https://llm.example.test/v1", true),
            ("http://studio:8080/v1", true),
            ("http://localhost/v1", true),
            ("ftp://llm.example.test", false),
            ("studio", false),
        ] {
            assert_eq!(server_address_shape(text).is_ok(), ok, "{text}");
        }
    }
}
