//! `GET /localapi/v0/status`: this computer's Tailscale, its network and the computers on it.
//!
//! Field names are those of Tailscale's `ipnstate.Status` and `ipnstate.PeerStatus` as of
//! v1.80.0 (`ipn/ipnstate/ipnstate.go`): Go writes them under their own names, so they are
//! spelled out here. A field this crate does not read is ignored, and one a version leaves out
//! reads as empty.

use crate::error::TailscaleError;
use crate::time::unix_seconds;
use porter_core::{Machine, MachineLink, MachineOwner, NodeId, UnixSeconds};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::net::IpAddr;

/// A Tailscale user's id: a number the control server gives a login.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserId(pub i64);

/// What tailscaled says it is doing (`ipn.State`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Backend {
    /// It has not started yet.
    NoState,
    /// Another user of this computer is signed in to it.
    InUseOtherUser,
    /// Nobody is signed in.
    NeedsLogin,
    /// Signed in, but an administrator has to approve this computer.
    NeedsMachineAuth,
    /// Signed in and turned off.
    Stopped,
    /// Connecting.
    Starting,
    /// Connected.
    Running,
    /// A state this version of porter does not know.
    Other,
}

impl Backend {
    /// The state a status names by its word (`"Running"`).
    pub fn from_word(word: &str) -> Self {
        match word {
            "NoState" => Backend::NoState,
            "InUseOtherUser" => Backend::InUseOtherUser,
            "NeedsLogin" => Backend::NeedsLogin,
            "NeedsMachineAuth" => Backend::NeedsMachineAuth,
            "Stopped" => Backend::Stopped,
            "Starting" => Backend::Starting,
            "Running" => Backend::Running,
            _ => Backend::Other,
        }
    }

    /// The state a watch notice names by its number (`ipn.State` as an integer).
    pub fn from_number(number: i64) -> Self {
        match number {
            0 => Backend::NoState,
            1 => Backend::InUseOtherUser,
            2 => Backend::NeedsLogin,
            3 => Backend::NeedsMachineAuth,
            4 => Backend::Stopped,
            5 => Backend::Starting,
            6 => Backend::Running,
            _ => Backend::Other,
        }
    }

    /// What this state means for a porter account.
    pub fn standing(self) -> Standing {
        match self {
            Backend::Running => Standing::Ready,
            Backend::NeedsLogin => Standing::SignedOut,
            Backend::Stopped | Backend::NeedsMachineAuth | Backend::InUseOtherUser => {
                Standing::Down
            }
            Backend::NoState | Backend::Starting | Backend::Other => Standing::Changing,
        }
    }
}

/// What tailscaled's state means for the account porter keeps for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Standing {
    /// Running and signed in.
    Ready,
    /// Nobody is signed in: the person has to sign in again, in Tailscale.
    SignedOut,
    /// Not connected, and not going to be until somebody acts (turned off, waiting for an
    /// administrator, another user's).
    Down,
    /// In the middle of changing: the next answer settles it, so nothing is concluded now.
    Changing,
}

/// A Tailscale login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// Its id.
    pub id: UserId,
    /// The login (`ada@example.org`).
    pub login: String,
    /// The name it is shown by.
    pub display_name: String,
}

/// One computer on the network, this one or a peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Its stable id.
    pub id: NodeId,
    /// The name of the machine as the operating system calls it.
    pub host_name: String,
    /// Its MagicDNS name without the dot at the end; empty when the network has no names.
    pub dns: String,
    /// Its operating system.
    pub os: String,
    /// The user that owns it.
    pub user: UserId,
    /// Its Tailscale addresses.
    pub addresses: Vec<IpAddr>,
    /// Whether it is connected now.
    pub online: bool,
    /// When it was last connected; none while online or when unknown.
    pub last_seen: Option<UnixSeconds>,
    /// Its tags (`tag:server`); empty for a computer of a person.
    pub tags: Vec<String>,
    /// Whether it was shared in from another person.
    pub shared: bool,
    /// The SSH host keys it advertises: present when Tailscale SSH is on.
    pub ssh_host_keys: Vec<String>,
}

impl Node {
    /// The name people call it by: the first part of its network name, else its host name.
    pub fn name(&self) -> &str {
        let first = self.dns.split('.').next().unwrap_or_default();
        match first.is_empty() {
            true => &self.host_name,
            false => first,
        }
    }

    /// Whose it is, as seen by the user `me`: tagged when it has a tag, shared when another
    /// person shared it in or another user owns it, else mine.
    pub fn owner(&self, me: UserId) -> MachineOwner {
        match (!self.tags.is_empty(), self.shared || self.user != me) {
            (true, _) => MachineOwner::Tagged,
            (false, true) => MachineOwner::Shared,
            (false, false) => MachineOwner::Mine,
        }
    }

    /// The row `Tailnet1.Machines` lists for it, as seen by the user `me`.
    pub fn machine(&self, me: UserId) -> Machine {
        let mut machine = Machine::new(
            self.id.clone(),
            self.name().to_owned(),
            self.dns.clone(),
            self.owner(me),
        )
        .with_addresses(self.addresses.clone())
        .with_os(self.os.clone())
        .with_link(MachineLink::of(self.online))
        .with_ssh_host_keys(self.ssh_host_keys.clone());
        machine.last_seen = self.last_seen.filter(|_| !self.online);
        machine
    }
}

/// The network this computer is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tailnet {
    /// Its name (`example.org`, or the login for a personal one).
    pub name: String,
}

/// This computer's Tailscale, as it says it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// What it is doing.
    pub backend: Backend,
    /// Tailscale's own sign-in page, while it waits for somebody to sign in.
    pub auth_url: Option<String>,
    /// The network, when signed in.
    pub tailnet: Option<Tailnet>,
    /// This computer, when signed in.
    pub me: Option<Node>,
    /// The other computers, in name order.
    pub peers: Vec<Node>,
    /// The users the nodes belong to.
    pub users: Vec<User>,
}

#[derive(Deserialize)]
struct RawStatus {
    #[serde(rename = "BackendState", default)]
    backend: String,
    #[serde(rename = "AuthURL", default)]
    auth_url: String,
    #[serde(rename = "Self", default)]
    me: Option<RawNode>,
    #[serde(rename = "Peer", default)]
    peers: Option<BTreeMap<String, RawNode>>,
    #[serde(rename = "User", default)]
    users: Option<BTreeMap<String, RawUser>>,
    #[serde(rename = "CurrentTailnet", default)]
    tailnet: Option<RawTailnet>,
}

#[derive(Deserialize)]
struct RawTailnet {
    #[serde(rename = "Name", default)]
    name: String,
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

#[derive(Deserialize)]
struct RawNode {
    #[serde(rename = "ID", default)]
    id: String,
    #[serde(rename = "HostName", default)]
    host_name: String,
    #[serde(rename = "DNSName", default)]
    dns: String,
    #[serde(rename = "OS", default)]
    os: String,
    #[serde(rename = "UserID", default)]
    user: i64,
    #[serde(rename = "AltSharerUserID", default)]
    sharer: i64,
    #[serde(rename = "TailscaleIPs", default)]
    addresses: Option<Vec<String>>,
    #[serde(rename = "Tags", default)]
    tags: Option<Vec<String>>,
    #[serde(rename = "Online", default)]
    online: bool,
    #[serde(rename = "LastSeen", default)]
    last_seen: String,
    #[serde(rename = "sshHostKeys", default)]
    ssh_host_keys: Option<Vec<String>>,
    #[serde(rename = "ShareeNode", default)]
    sharee: bool,
}

impl RawNode {
    /// The node, or none when its id is not one (such a computer cannot be keyed).
    fn read(self) -> Option<Node> {
        Some(Node {
            id: NodeId::parse(&self.id).ok()?,
            host_name: self.host_name,
            dns: self.dns.trim_end_matches('.').to_owned(),
            os: self.os,
            user: UserId(self.user),
            addresses: self
                .addresses
                .unwrap_or_default()
                .iter()
                .filter_map(|text| text.parse().ok())
                .collect(),
            online: self.online,
            last_seen: unix_seconds(&self.last_seen),
            tags: self.tags.unwrap_or_default(),
            shared: self.sharee || self.sharer != 0,
            ssh_host_keys: self.ssh_host_keys.unwrap_or_default(),
        })
    }
}

impl Status {
    /// Reads the body of a status answer.
    pub fn parse(body: &[u8]) -> Result<Self, TailscaleError> {
        let raw: RawStatus = crate::object::object(body)?;
        let mut peers: Vec<Node> = raw
            .peers
            .unwrap_or_default()
            .into_values()
            .filter_map(RawNode::read)
            .collect();
        peers.sort_by(|a, b| (a.name(), &a.id).cmp(&(b.name(), &b.id)));
        let users = raw
            .users
            .unwrap_or_default()
            .into_values()
            .map(|user| User {
                id: UserId(user.id),
                login: user.login,
                display_name: user.display_name,
            })
            .collect();
        Ok(Self {
            backend: Backend::from_word(&raw.backend),
            auth_url: Some(raw.auth_url).filter(|url| !url.is_empty()),
            tailnet: raw
                .tailnet
                .map(|tailnet| Tailnet { name: tailnet.name })
                .filter(|tailnet| !tailnet.name.is_empty()),
            me: raw.me.and_then(RawNode::read),
            peers,
            users,
        })
    }

    /// What the state means for the account.
    pub fn standing(&self) -> Standing {
        self.backend.standing()
    }

    /// The user with this id.
    pub fn user(&self, id: UserId) -> Option<&User> {
        self.users.iter().find(|user| user.id == id)
    }

    /// The signed-in user of this computer.
    pub fn me_user(&self) -> Option<&User> {
        self.me.as_ref().and_then(|me| self.user(me.user))
    }

    /// The name an account of this network is shown by: the login and the network
    /// (`ada@example.org on example.org`), or the login alone when the network is named after
    /// it (a personal one) or has no name. None while nobody is signed in.
    pub fn label(&self) -> Option<String> {
        let login = self.me_user()?.login.clone();
        Some(match self.tailnet.as_ref().map(|t| t.name.as_str()) {
            Some(name) if name != login => format!("{login} on {name}"),
            _ => login,
        })
    }

    /// The other computers, as `Tailnet1.Machines` lists them; none while this computer is not
    /// known (nobody signed in), since whose a computer is depends on who is asking.
    pub fn machines(&self) -> Vec<Machine> {
        match &self.me {
            Some(me) => self
                .peers
                .iter()
                .map(|peer| peer.machine(me.user))
                .collect(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(id: &str, name: &str, user: i64) -> serde_json::Value {
        json!({
            "ID": id, "HostName": name, "DNSName": format!("{name}.tail1234.ts.net."),
            "OS": "linux", "UserID": user, "TailscaleIPs": ["100.64.0.2", "fd7a:115c:a1e0::2"],
            "Online": true, "LastSeen": "0001-01-01T00:00:00Z",
        })
    }

    fn running() -> serde_json::Value {
        let mut offline = node("nOFF", "old-laptop", 1);
        offline["Online"] = json!(false);
        offline["LastSeen"] = json!("2026-09-21T14:13:20.5Z");
        let mut tagged = node("nTAG", "build-box", 1);
        tagged["Tags"] = json!(["tag:ci"]);
        let mut shared = node("nSHR", "friends-pc", 2);
        shared["ShareeNode"] = json!(true);
        let mut ssh = node("nSSH", "pi", 1);
        ssh["sshHostKeys"] = json!(["ssh-ed25519 AAAA"]);
        json!({
            "Version": "1.80.0", "BackendState": "Running", "AuthURL": "",
            "Self": node("nSELF", "desk", 1),
            "CurrentTailnet": {"Name": "ada@example.org", "MagicDNSSuffix": "tail1234.ts.net"},
            "Peer": {"k1": offline, "k2": tagged, "k3": shared, "k4": ssh, "k5": node("nSAME", "other-user", 3)},
            "User": {
                "1": {"ID": 1, "LoginName": "ada@example.org", "DisplayName": "Ada"},
                "2": {"ID": 2, "LoginName": "bob@example.net", "DisplayName": "Bob"},
            },
        })
    }

    fn status(value: &serde_json::Value) -> Status {
        Status::parse(value.to_string().as_bytes()).expect("status")
    }

    #[test]
    fn the_states_mean_what_they_say() {
        let table = [
            ("Running", Standing::Ready),
            ("NeedsLogin", Standing::SignedOut),
            ("Stopped", Standing::Down),
            ("NeedsMachineAuth", Standing::Down),
            ("InUseOtherUser", Standing::Down),
            ("Starting", Standing::Changing),
            ("NoState", Standing::Changing),
            ("SomethingNew", Standing::Changing),
        ];
        for (word, want) in table {
            assert_eq!(Backend::from_word(word).standing(), want, "{word}");
        }
        for (number, word) in [
            (2, "NeedsLogin"),
            (4, "Stopped"),
            (6, "Running"),
            (5, "Starting"),
        ] {
            assert_eq!(
                Backend::from_number(number),
                Backend::from_word(word),
                "{number}"
            );
        }
        assert_eq!(Backend::from_number(99), Backend::Other);
    }

    #[test]
    fn a_running_status_lists_the_peers_and_not_this_computer() {
        let status = status(&running());
        assert_eq!(status.standing(), Standing::Ready);
        let machines = status.machines();
        let names: Vec<&str> = machines.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            ["build-box", "friends-pc", "old-laptop", "other-user", "pi"]
        );
        assert!(machines.iter().all(|m| m.node.as_str() != "nSELF"));
        let by = |name: &str| machines.iter().find(|m| m.name == name).expect(name);
        assert_eq!(by("pi").dns, "pi.tail1234.ts.net", "no dot at the end");
        assert_eq!(by("pi").addresses.len(), 2);
        assert_eq!(by("pi").owner, MachineOwner::Mine);
        assert!(by("pi").ssh.is_on());
        assert_eq!(by("pi").ssh_host_keys, ["ssh-ed25519 AAAA"]);
        assert!(!by("old-laptop").ssh.is_on());
        assert_eq!(by("build-box").owner, MachineOwner::Tagged);
        assert_eq!(by("friends-pc").owner, MachineOwner::Shared);
        // Another user's node on the same network is shared, not mine.
        assert_eq!(by("other-user").owner, MachineOwner::Shared);
    }

    #[test]
    fn last_seen_is_given_for_an_offline_machine_only() {
        let machines = status(&running()).machines();
        let old = machines
            .iter()
            .find(|m| m.name == "old-laptop")
            .expect("old");
        assert!(!old.link.is_online());
        assert_eq!(old.last_seen, Some(UnixSeconds(1_790_000_000)));
        let pi = machines.iter().find(|m| m.name == "pi").expect("pi");
        assert_eq!(pi.last_seen, None, "online, and Go's zero time is never");
        // An online peer is online even if the control server left a stale time.
        let mut stale = running();
        stale["Peer"]["k4"]["LastSeen"] = json!("2026-09-21T14:13:20Z");
        let machines = status(&stale).machines();
        let pi = machines.iter().find(|m| m.name == "pi").expect("pi");
        assert_eq!(pi.last_seen, None);
    }

    #[test]
    fn the_label_is_the_login_and_the_network() {
        assert_eq!(
            status(&running()).label().as_deref(),
            Some("ada@example.org"),
            "a personal network is named after the login"
        );
        let mut company = running();
        company["CurrentTailnet"]["Name"] = json!("example.org");
        assert_eq!(
            status(&company).label().as_deref(),
            Some("ada@example.org on example.org")
        );
        let mut unnamed = running();
        unnamed["CurrentTailnet"] = json!(null);
        assert_eq!(status(&unnamed).label().as_deref(), Some("ada@example.org"));
    }

    #[test]
    fn a_signed_out_status_has_no_computers_and_the_sign_in_page() {
        let out = status(&json!({
            "Version": "1.80.0", "BackendState": "NeedsLogin",
            "AuthURL": "https://login.tailscale.com/a/0123456789abcdef",
            "Peer": null, "User": null, "Self": null,
        }));
        assert_eq!(out.standing(), Standing::SignedOut);
        assert_eq!(
            out.auth_url.as_deref(),
            Some("https://login.tailscale.com/a/0123456789abcdef")
        );
        assert!(out.machines().is_empty());
        assert_eq!(out.label(), None);
    }

    #[test]
    fn a_peer_without_a_usable_id_is_left_out_not_fatal() {
        let mut value = running();
        value["Peer"]["bad"] = node("not an id", "bad", 1);
        assert_eq!(status(&value).machines().len(), 5);
    }

    #[test]
    fn what_is_not_a_status_is_malformed() {
        for body in ["", "not json", "[]", "\"Running\"", "{\"Peer\": 3}"] {
            assert_eq!(
                Status::parse(body.as_bytes()),
                Err(TailscaleError::Malformed),
                "{body:?}"
            );
        }
        // A bare object is an empty status: a future version that drops everything is not an
        // error, it just lists nobody.
        assert!(Status::parse(b"{}").expect("empty").machines().is_empty());
    }
}
