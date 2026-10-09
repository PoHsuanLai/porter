//! The person's computers on their Tailscale network, as `org.quire.Tailnet1` hands them to the
//! shell, Settings and temor: a machine is keyed by its Tailscale node id, which Tailscale keeps
//! across renames and address changes. Pure data; the one program that reads Tailscale is
//! `porter-tailscale`.

use crate::error::CoreError;
use crate::units::UnixSeconds;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::IpAddr;

/// The longest node id read. Tailscale's are a dozen characters (`nPXS7a2CNTRL`).
const NODE_ID_MAX: usize = 64;

/// A Tailscale node's STABLE id (Tailscale's `StableNodeID`, the string `ID` of its status and
/// `StableID` of its whois, such as `nPXS7a2CNTRL`): it stays the same when the computer is
/// renamed, gets another address or a new key. It is never the numeric node id or the node key,
/// which change. Approval, SSH pins and consent are kept by it, never by name or address.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NodeId(Box<str>);

impl NodeId {
    /// The node id written as `text`: 1 to 64 letters, digits, `-` or `_`.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let good = !text.is_empty()
            && text.len() <= NODE_ID_MAX
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        match good {
            true => Ok(Self(text.into())),
            false => Err(CoreError::MalformedId {
                what: "node id",
                text: text.chars().take(80).collect(),
            }),
        }
    }

    /// The id as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NodeId {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<NodeId> for String {
    fn from(id: NodeId) -> String {
        id.0.into()
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whose a machine is. A word from a closed set, since a tagged machine is a third case that a
/// yes or no cannot say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineOwner {
    /// The person's own: their login made it, and nobody shared it.
    Mine,
    /// Another person's, shared with this person or on the same network.
    Shared,
    /// Run by a tag (a server the organisation set up), not by any one person.
    Tagged,
}

impl MachineOwner {
    /// Every owner, for tables.
    pub const ALL: [MachineOwner; 3] = [
        MachineOwner::Mine,
        MachineOwner::Shared,
        MachineOwner::Tagged,
    ];

    /// The word on the bus.
    pub fn slug(self) -> &'static str {
        match self {
            MachineOwner::Mine => "mine",
            MachineOwner::Shared => "shared",
            MachineOwner::Tagged => "tagged",
        }
    }

    /// The owner a word stands for.
    pub fn from_slug(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|owner| owner.slug() == text)
    }
}

/// One computer on the person's Tailscale network, as `Tailnet1.Machines` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    /// Its stable id.
    pub node: NodeId,
    /// The name people call it by (the first part of its network name).
    pub name: String,
    /// Its MagicDNS name without the dot at the end: what to connect to.
    pub dns: String,
    /// Its Tailscale addresses.
    pub addresses: Vec<IpAddr>,
    /// Its operating system as Tailscale says it (`linux`, `macOS`); empty when unknown.
    pub os: String,
    /// Whether it is connected now.
    pub online: bool,
    /// When it was last connected: absent while it is online, and when Tailscale does not know.
    pub last_seen: Option<UnixSeconds>,
    /// Whether Tailscale's own SSH is on (the machine advertises SSH host keys).
    pub ssh: bool,
    /// The SSH host keys it advertises.
    pub ssh_host_keys: Vec<String>,
    /// Whose it is.
    pub owner: MachineOwner,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_are_short_plain_words() {
        for good in ["nPXS7a2CNTRL", "n-1_2", &"a".repeat(64)] {
            assert!(NodeId::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "a b", "n/1", "n.1", "ü", &"a".repeat(65), "n\n1"] {
            assert!(NodeId::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_node_id_is_its_string_on_the_wire() {
        let id = NodeId::parse("nPXS7a2CNTRL").expect("id");
        assert_eq!(
            serde_json::to_string(&id).expect("json"),
            "\"nPXS7a2CNTRL\""
        );
        assert!(serde_json::from_str::<NodeId>("\"bad id\"").is_err());
    }

    #[test]
    fn an_owner_is_its_serde_word() {
        for owner in MachineOwner::ALL {
            let json = serde_json::to_value(owner).expect("json");
            assert_eq!(json.as_str(), Some(owner.slug()));
            assert_eq!(MachineOwner::from_slug(owner.slug()), Some(owner));
        }
        assert_eq!(MachineOwner::from_slug("Mine"), None);
        assert_eq!(MachineOwner::from_slug(""), None);
    }
}
