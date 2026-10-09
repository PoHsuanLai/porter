//! This computer, as its own Tailscale says it is.

use porter_core::NodeId;
use porter_tailscale::{Standing, Status, UserId};
use std::net::IpAddr;

/// Who this computer is on the network: the id Tailscale keeps for it, the user it belongs to,
/// the name people call it by and the addresses the network gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    node: NodeId,
    user: UserId,
    name: String,
    addresses: Vec<IpAddr>,
}

impl Identity {
    /// An identity of these parts.
    pub fn new(node: NodeId, user: UserId, name: String, addresses: Vec<IpAddr>) -> Self {
        Self {
            node,
            user,
            name,
            addresses,
        }
    }

    /// This computer as `status` says, when Tailscale is running and signed in; none otherwise.
    pub fn of(status: &Status) -> Option<Self> {
        if status.standing() != Standing::Ready {
            return None;
        }
        let me = status.me.as_ref()?;
        Some(Self::new(
            me.id.clone(),
            me.user,
            me.name().to_owned(),
            me.addresses.clone(),
        ))
    }

    /// Its stable id.
    pub fn node(&self) -> &NodeId {
        &self.node
    }

    /// The user it belongs to.
    pub fn user(&self) -> UserId {
        self.user
    }

    /// The name people call it by.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The addresses the network gave it.
    pub fn addresses(&self) -> &[IpAddr] {
        &self.addresses
    }

    /// Whether `ip` is one of its own addresses.
    pub fn owns(&self, ip: IpAddr) -> bool {
        self.addresses.contains(&ip)
    }
}
