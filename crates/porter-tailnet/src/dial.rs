//! Reaching one of the person's own computers, after asking Tailscale who is at the address.
//!
//! [`Dialer::connect`] looks up the computer by its stable id in what Tailscale says now (so a
//! computer that got a new address is still found), asks Tailscale who has the address it is
//! about to dial, and goes on only when that is the same computer, still the person's own (not
//! tagged, not shared in, not another user's). The connection is made from this computer's own
//! network address: it goes out on the network or not at all. This is done for every
//! connection, not once.

use porter_core::{MachineOwner, NodeId};
use porter_tailscale::{LocalApi, Standing, TailscaleError, UserId, WhoIs};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::net::{TcpSocket, TcpStream};

/// How long one connection may take to be made before it is given up as not answering. Far
/// beyond the time a computer that is up needs, and below the time the system itself would wait.
const PATIENCE: Duration = Duration::from_secs(60);

/// Why a computer was not reached. `Display` is the plain sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DialError {
    /// Tailscale could not be asked, or is not running and signed in.
    #[error("{0}")]
    Tailscale(TailscaleError),
    /// There is no such computer on the network now.
    #[error("That computer is not on your Tailscale network any more.")]
    NoSuchComputer,
    /// It is not the person's own.
    #[error("That computer is not one of your own.")]
    NotYours(MachineOwner),
    /// It is switched off or out of reach.
    #[error("That computer is switched off or out of reach.")]
    Offline,
    /// The address is now another computer's.
    #[error("That computer's place on the network now belongs to another computer.")]
    Changed,
    /// Nothing answered.
    #[error("That computer did not answer.")]
    Unreachable,
}

/// Opens connections to the person's own computers.
#[derive(Debug, Clone)]
pub struct Dialer {
    api: LocalApi,
    port: u16,
    patience: Duration,
}

/// IPv4 first: it is the address every Tailscale has.
fn v4_first(mut addresses: Vec<IpAddr>) -> Vec<IpAddr> {
    addresses.sort_by_key(|ip| ip.is_ipv6());
    addresses
}

impl Dialer {
    /// A dialer that asks the Tailscale `api` reaches and connects to `port`.
    pub fn new(api: LocalApi, port: u16) -> Self {
        Self {
            api,
            port,
            patience: PATIENCE,
        }
    }

    /// The same dialer giving up on a connection that is not made within `patience`.
    #[must_use]
    pub fn patient(self, patience: Duration) -> Self {
        Self { patience, ..self }
    }

    /// A connection to the computer `node`, once Tailscale has said that is who is there.
    pub async fn connect(&self, node: &NodeId) -> Result<TcpStream, DialError> {
        let status = self.api.status().await.map_err(DialError::Tailscale)?;
        let me = match status.standing() {
            Standing::Ready => status.me.clone(),
            Standing::SignedOut => return Err(DialError::Tailscale(TailscaleError::SignedOut)),
            Standing::Down | Standing::Changing => {
                return Err(DialError::Tailscale(TailscaleError::NotRunning));
            }
        }
        .ok_or(DialError::Tailscale(TailscaleError::Malformed))?;
        let peer = status
            .peers
            .iter()
            .find(|peer| peer.id == *node)
            .ok_or(DialError::NoSuchComputer)?;
        match peer.owner(me.user) {
            MachineOwner::Mine => {}
            other => return Err(DialError::NotYours(other)),
        }
        if !peer.online {
            return Err(DialError::Offline);
        }
        let mut last = DialError::Unreachable;
        for address in v4_first(peer.addresses.clone()) {
            // Out of this computer's own address of the same kind, so that it leaves on the
            // network and nowhere else.
            let Some(source) = me
                .addresses
                .iter()
                .find(|mine| mine.is_ipv4() == address.is_ipv4())
            else {
                continue;
            };
            let to = SocketAddr::new(address, self.port);
            let who = self.api.whois(to).await.map_err(|e| match e {
                TailscaleError::NoSuchPeer => DialError::Changed,
                other => DialError::Tailscale(other),
            })?;
            if !is_the_one(&who, node, address, me.user) {
                return Err(DialError::Changed);
            }
            last = match tokio::time::timeout(self.patience, connect(*source, to)).await {
                Ok(Ok(stream)) => return Ok(stream),
                Ok(Err(_)) | Err(_) => DialError::Unreachable,
            };
        }
        Err(last)
    }
}

/// Whether what Tailscale says of `address` is the computer `node`, still the person's own
/// (`me` is the person): the same id, the address among its own, no tag, nobody's but theirs.
fn is_the_one(who: &WhoIs, node: &NodeId, address: IpAddr, me: UserId) -> bool {
    who.node.id == *node
        && who.node.addresses.contains(&address)
        && who.owner(me) == MachineOwner::Mine
}

async fn connect(from: IpAddr, to: SocketAddr) -> std::io::Result<TcpStream> {
    let socket = match to {
        SocketAddr::V4(_) => TcpSocket::new_v4()?,
        SocketAddr::V6(_) => TcpSocket::new_v6()?,
    };
    socket.bind(SocketAddr::new(from, 0))?;
    socket.connect(to).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_tailscale::{User, WhoIsNode};

    const ME: UserId = UserId(1);

    fn node(text: &str) -> NodeId {
        NodeId::parse(text).expect("a node id")
    }

    fn who(id: &str, user: i64, address: &str) -> WhoIs {
        WhoIs {
            node: WhoIsNode {
                id: node(id),
                dns: String::new(),
                user: UserId(user),
                sharer: None,
                tags: Vec::new(),
                addresses: vec![address.parse().unwrap()],
            },
            user: User {
                id: UserId(user),
                login: String::new(),
                display_name: String::new(),
            },
        }
    }

    #[test]
    fn only_the_same_computer_of_the_same_person_is_the_one() {
        let at = "100.64.0.2".parse().unwrap();
        assert!(is_the_one(
            &who("nPI", 1, "100.64.0.2"),
            &node("nPI"),
            at,
            ME
        ));
        // The address is now another computer's.
        assert!(!is_the_one(
            &who("nOTHER", 1, "100.64.0.2"),
            &node("nPI"),
            at,
            ME
        ));
        // Tailscale does not list the address among the computer's own.
        assert!(!is_the_one(
            &who("nPI", 1, "100.64.0.9"),
            &node("nPI"),
            at,
            ME
        ));
        // It became a tagged server, another person's, or shared in.
        let mut tagged = who("nPI", 1, "100.64.0.2");
        tagged.node.tags.push("tag:ci".into());
        assert!(!is_the_one(&tagged, &node("nPI"), at, ME));
        assert!(!is_the_one(
            &who("nPI", 2, "100.64.0.2"),
            &node("nPI"),
            at,
            ME
        ));
        let mut shared = who("nPI", 1, "100.64.0.2");
        shared.node.sharer = Some(UserId(2));
        assert!(!is_the_one(&shared, &node("nPI"), at, ME));
    }

    #[test]
    fn the_sentences_are_plain() {
        for fault in [
            DialError::Tailscale(TailscaleError::NotRunning),
            DialError::NoSuchComputer,
            DialError::NotYours(MachineOwner::Shared),
            DialError::Offline,
            DialError::Changed,
            DialError::Unreachable,
        ] {
            let text = fault.to_string();
            assert!(text.ends_with('.'), "{text}");
            let lower = text.to_lowercase();
            for word in ["porter", "inferd", "socket", "node", "tailnet", "address"] {
                assert!(!lower.contains(word), "{text}");
            }
        }
    }
}
