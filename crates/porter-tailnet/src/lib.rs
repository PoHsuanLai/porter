//! Lending a computer's models to the person's other computers over their Tailscale network,
//! and calling the models of another.
//!
//! **Who may ask** ([`judge`]) is decided from what Tailscale itself says about the computer a
//! request came from, never from what the request says: the computer must be the person's own
//! (their login made it, it is not a tagged server and nobody shared it in), it must not be
//! this computer (a program of another account on this computer reaches the listener through
//! the network address and would otherwise pass as "your computer"), and the person must not
//! have said no to it. A computer that fails the first test may still be let in, but only by
//! the person's own word given ahead of time in Settings ([`Guests::set`]); it is never asked
//! about. A computer of the person's own that they have not answered about yet is asked about
//! once, the first time it wants to use a model ([`Guests::ask`]). Everything is kept by
//! Tailscale's stable computer id ([`porter_core::NodeId`]), never by name or address.
//!
//! The pure parts, [`judge`], [`Guests`], [`Hello`] and [`Refusal`], reach no socket. With the
//! `io` feature the crate also holds the rest of the work:
//!
//! * [`Observer`] follows this computer's Tailscale (the notices it streams, with a look behind
//!   them) and says what this computer is now;
//! * [`lend`] listens on this computer's network addresses, and only those, follows them as
//!   they change, judges every connection, and hands the ones that pass to a [`Visits`]
//!   handler;
//! * [`Dialer`] opens a connection to one of the person's computers after asking Tailscale who
//!   is at the address, and [`Relay`] gives a program that can only dial a local path a way to
//!   one such computer, checking again on every connection;
//! * [`greet`] reads a computer's [`Hello`].
//!
//! The crate takes its configuration (the port, which addresses may be listened on, the
//! limits) from its caller and reads no environment.

mod guests;
mod hello;
mod identity;
mod judge;
mod refusal;

#[cfg(feature = "io")]
mod dial;
#[cfg(feature = "io")]
mod greet;
#[cfg(feature = "io")]
mod lend;
#[cfg(feature = "io")]
mod observe;
#[cfg(feature = "io")]
mod relay;

pub use guests::{
    Ask, AskOutcome, Guest, GuestAnswer, GuestError, GuestEvent, GuestRow, Guests, RowState, State,
};
pub use hello::{Approval, HELLO_PATH, Hello, LentModel};
pub use identity::Identity;
pub use judge::{Footing, Peer, Welcome, judge};
pub use refusal::Refusal;

#[cfg(feature = "io")]
pub use dial::{DialError, Dialer};
#[cfg(feature = "io")]
pub use greet::{GreetError, greet};
#[cfg(feature = "io")]
pub use lend::{Config, Lending, Limits, Listening, Permit, Visit, Visits, lend};
#[cfg(feature = "io")]
pub use observe::{Observer, Seen, Timing};
#[cfg(feature = "io")]
pub use relay::{OnFault, Relay};

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The one port a computer lends its models on, the same on every computer, so that a computer
/// is looked for without being told where.
///
/// Why this number: it is above 1024 (no privilege is needed), below the 32768 where Linux
/// hands out the ports of outgoing connections (a connection of this computer's own can never
/// have taken it), and it is none of the ports that model servers and the programs around
/// them are known to use (11434 Ollama, 1234 LM Studio, 8080 llama.cpp, 8000 vLLM, 5000, 3000).
/// It is not in IANA's registry as far as could be read, which was not checked against the live
/// registry. It is only ever listened on at the network's own addresses, never at "any
/// address".
pub const PORT: u16 = 26434;

/// Whether `ip` is an address Tailscale gives a computer: `100.64.0.0/10` for IPv4, the
/// `fd7a:115c:a1e0::/48` block for IPv6. What is listened on in a product is never anything
/// else, whatever Tailscale reports.
pub fn is_network_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => in_v4(v4),
        IpAddr::V6(v6) => in_v6(v6),
    }
}

fn in_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (64..=127).contains(&b)
}

fn in_v6(ip: Ipv6Addr) -> bool {
    ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_networks_own_addresses_are_its_addresses() {
        let yes = ["100.64.0.1", "100.127.255.254", "fd7a:115c:a1e0::1"];
        let no = [
            "0.0.0.0",
            "127.0.0.1",
            "100.63.255.255",
            "100.128.0.0",
            "192.168.1.5",
            "::",
            "::1",
            "fd7a:115c:a1e1::1",
            "fe80::1",
        ];
        for text in yes {
            assert!(is_network_address(text.parse().unwrap()), "{text}");
        }
        for text in no {
            assert!(!is_network_address(text.parse().unwrap()), "{text}");
        }
    }
}
