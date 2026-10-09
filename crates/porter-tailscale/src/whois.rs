//! `GET /localapi/v0/whois?addr=<ip>[:port]`: who a tailnet address belongs to.
//!
//! Field names are those of Tailscale's `apitype.WhoIsResponse`, `tailcfg.Node` and
//! `tailcfg.UserProfile` as of v1.80.0 (`client/tailscale/apitype/apitype.go`,
//! `tailcfg/tailcfg.go`): Go writes them under their own names. The node's `Addresses` are
//! prefixes (`100.64.0.2/32`).

use crate::error::TailscaleError;
use crate::status::{User, UserId};
use porter_core::{MachineOwner, NodeId};
use serde::Deserialize;
use std::net::IpAddr;

/// The computer behind an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhoIsNode {
    /// Its stable id.
    pub id: NodeId,
    /// Its MagicDNS name without the dot at the end.
    pub dns: String,
    /// The user that made it. With tags this is not who runs it.
    pub user: UserId,
    /// Who shared it in, when somebody did.
    pub sharer: Option<UserId>,
    /// Its tags.
    pub tags: Vec<String>,
    /// Its Tailscale addresses.
    pub addresses: Vec<IpAddr>,
}

/// What Tailscale says of the owner of an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhoIs {
    /// The computer.
    pub node: WhoIsNode,
    /// The user it belongs to.
    pub user: User,
}

impl WhoIs {
    /// Whose the computer is, as seen by the user `me`: the same rule as a listed computer's.
    pub fn owner(&self, me: UserId) -> MachineOwner {
        let node = &self.node;
        match (
            !node.tags.is_empty(),
            node.sharer.is_some() || node.user != me,
        ) {
            (true, _) => MachineOwner::Tagged,
            (false, true) => MachineOwner::Shared,
            (false, false) => MachineOwner::Mine,
        }
    }
}

#[derive(Deserialize)]
struct RawWhoIs {
    #[serde(rename = "Node")]
    node: Option<RawNode>,
    #[serde(rename = "UserProfile")]
    user: Option<RawUser>,
}

#[derive(Deserialize)]
struct RawNode {
    #[serde(rename = "StableID", default)]
    id: String,
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "User", default)]
    user: i64,
    #[serde(rename = "Sharer", default)]
    sharer: i64,
    #[serde(rename = "Tags", default)]
    tags: Option<Vec<String>>,
    #[serde(rename = "Addresses", default)]
    addresses: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RawUser {
    #[serde(rename = "ID", default)]
    id: i64,
    #[serde(rename = "LoginName", default)]
    login: String,
    #[serde(rename = "DisplayName", default)]
    display_name: String,
}

impl WhoIs {
    /// Reads the body of a whois answer. Tailscale says both the node and the user are present
    /// in an answer that succeeded; one that has neither, or an id that is not one, is
    /// malformed.
    pub fn parse(body: &[u8]) -> Result<Self, TailscaleError> {
        let raw: RawWhoIs = crate::object::object(body)?;
        let (Some(node), Some(user)) = (raw.node, raw.user) else {
            return Err(TailscaleError::Malformed);
        };
        Ok(Self {
            node: WhoIsNode {
                id: NodeId::parse(&node.id).map_err(|_| TailscaleError::Malformed)?,
                dns: node.name.trim_end_matches('.').to_owned(),
                user: UserId(node.user),
                sharer: Some(node.sharer).filter(|id| *id != 0).map(UserId),
                tags: node.tags.unwrap_or_default(),
                addresses: node
                    .addresses
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|prefix| prefix.split('/').next()?.parse().ok())
                    .collect(),
            },
            user: User {
                id: UserId(user.id),
                login: user.login,
                display_name: user.display_name,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn answer(node: serde_json::Value) -> Vec<u8> {
        json!({
            "Node": node,
            "UserProfile": {"ID": 1, "LoginName": "ada@example.org", "DisplayName": "Ada"},
            "CapMap": {},
        })
        .to_string()
        .into_bytes()
    }

    fn base() -> serde_json::Value {
        json!({
            "ID": 12345, "StableID": "nPXS7a2CNTRL", "Name": "pi.tail1234.ts.net.", "User": 1,
            "Addresses": ["100.64.0.2/32", "fd7a:115c:a1e0::2/128"], "Online": true,
        })
    }

    #[test]
    fn a_whois_answer_reads() {
        let who = WhoIs::parse(&answer(base())).expect("whois");
        assert_eq!(who.node.id.as_str(), "nPXS7a2CNTRL");
        assert_eq!(who.node.dns, "pi.tail1234.ts.net");
        assert_eq!(who.node.addresses.len(), 2);
        assert_eq!(who.node.addresses[0].to_string(), "100.64.0.2");
        assert_eq!(who.user.login, "ada@example.org");
        assert_eq!(who.owner(UserId(1)), MachineOwner::Mine);
        assert_eq!(who.owner(UserId(9)), MachineOwner::Shared);
    }

    #[test]
    fn a_tag_or_a_sharer_decides_the_owner_before_the_user_does() {
        let mut tagged = base();
        tagged["Tags"] = json!(["tag:server"]);
        let who = WhoIs::parse(&answer(tagged)).expect("whois");
        assert_eq!(who.owner(UserId(1)), MachineOwner::Tagged);
        let mut shared = base();
        shared["Sharer"] = json!(7);
        let who = WhoIs::parse(&answer(shared)).expect("whois");
        assert_eq!(who.node.sharer, Some(UserId(7)));
        assert_eq!(who.owner(UserId(1)), MachineOwner::Shared);
    }

    #[test]
    fn an_answer_without_both_halves_is_malformed() {
        for body in [
            "".to_owned(),
            "{}".to_owned(),
            json!({"Node": base()}).to_string(),
            json!({"UserProfile": {"ID": 1}}).to_string(),
            String::from_utf8(answer(json!({"StableID": "not an id", "User": 1}))).expect("text"),
        ] {
            assert_eq!(
                WhoIs::parse(body.as_bytes()),
                Err(TailscaleError::Malformed),
                "{body}"
            );
        }
    }
}
