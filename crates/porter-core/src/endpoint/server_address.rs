//! Which address the person may give for an AI server of their own (an OpenAI-compatible
//! server: a llama.cpp or vLLM on the same network, a gateway, a company's proxy).
//!
//! The rule, pure and with no environment read:
//!
//! * `https` is accepted for any host;
//! * plain `http` is accepted for this computer (`Origin::is_loopback`) and for one of the
//!   person's own computers on their tailnet: a [`Machine`] whose owner is
//!   [`MachineOwner::Mine`], matched by its name, its network name or one of its addresses.
//!   Another person's computer, a tagged server and every other host need `https`.
//!
//! The list of computers is an input: the caller reads it from wherever it has one (accountd's
//! `Tailnet1.Machines`) and passes it in; a caller with none passes `&[]` and so accepts plain
//! `http` for this computer only.

use super::{EndpointUrl, Origin, Tls, UrlScheme};
use crate::tailnet::{Machine, MachineOwner};
use std::net::IpAddr;

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

/// The address `text` stands for, or why it is refused; `own` is the person's computers on their
/// tailnet, as far as the caller knows them.
///
/// # Errors
/// [`ServerAddressFault::NotAnAddress`] for text that is not an `http` or `https` address;
/// [`ServerAddressFault::PlainOffTrusted`] for `http` to a host that is not this computer nor one
/// of the person's own computers.
pub fn server_address(text: &str, own: &[Machine]) -> Result<ServerAddress, ServerAddressFault> {
    let url = EndpointUrl::parse(text.trim()).map_err(|_| ServerAddressFault::NotAnAddress)?;
    let origin = url.origin();
    match origin.scheme {
        UrlScheme::Https => Ok(ServerAddress {
            url,
            tls: Tls::Implicit,
        }),
        UrlScheme::Http if origin.is_loopback() || (origin.is_tailnet() && is_own(&origin, own)) => Ok(ServerAddress {
            url,
            tls: Tls::Plain,
        }),
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

/// Whether the origin's host is one of the person's own computers.
fn is_own(origin: &Origin, own: &[Machine]) -> bool {
    let host = origin.host.as_str();
    let address: Option<IpAddr> = host.parse().ok();
    own.iter()
        .filter(|machine| machine.owner == MachineOwner::Mine)
        .any(|machine| {
            host.eq_ignore_ascii_case(&machine.name)
                || host.eq_ignore_ascii_case(&machine.dns)
                || address.is_some_and(|ip| machine.addresses.contains(&ip))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tailnet::NodeId;

    fn machine(name: &str, dns: &str, address: &str, owner: MachineOwner) -> Machine {
        Machine::new(
            NodeId::parse("nTEST1").expect("node id"),
            name.to_owned(),
            dns.to_owned(),
            owner,
        )
        .with_addresses(vec![address.parse().expect("address")])
    }

    #[test]
    fn an_address_is_accepted_by_its_scheme_and_its_host() {
        let own = [
            machine(
                "studio",
                "studio.tail1.ts.net",
                "100.64.0.7",
                MachineOwner::Mine,
            ),
            machine(
                "friend",
                "friend.tail1.ts.net",
                "100.64.0.8",
                MachineOwner::Shared,
            ),
            machine(
                "gpu-box",
                "gpu-box.tail1.ts.net",
                "100.64.0.9",
                MachineOwner::Tagged,
            ),
        ];
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
            let got = server_address(text, &own).map(|address| address.tls());
            assert_eq!(got, *want, "{what}: {text}");
        }
        // With no list of computers only this computer may be plain.
        assert_eq!(
            server_address("http://studio:8080/v1", &[]),
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
            let address = server_address(text, &[]).expect(text);
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
