//! A computer on a fake Tailscale network: the fake LocalAPI it asks, the fake `Tailnet1` accountd
//! lists its computers on, and the addresses of its own. Every address is a loopback address of
//! the test (`127.x.y.z`, from the process id), so a socket bound to one is reached from a socket
//! bound to another, inside the jail, and nothing leaves the computer. Nothing here touches the
//! real Tailscale or `/var/run/tailscale`.

use porter_dbus::{Details, machine_to_dbus};
use porter_fake_servers::{Daemon, FakeLocalApi, FakePeer, Network};
use porter_tailscale::LocalApi;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use zbus::fdo;

/// What a test says of one computer's side of the network.
pub struct TailnetPlan {
    /// What its Tailscale says.
    pub network: Network,
    /// The port computers lend on (the same on both).
    pub port: u16,
    /// Whether `ai.tailnet.serve` is on from the start.
    pub serving: bool,
}

/// A running fake Tailscale and the bus object that lists its computers.
#[derive(Debug)]
pub struct FakeTailscale {
    pub fake: FakeLocalApi,
    pub api: LocalApi,
    /// Whether Tailscale is an account in the accounts (accountd lists no computers without).
    pub account: Arc<AtomicBool>,
}

impl FakeTailscale {
    pub async fn start(dir: &std::path::Path, network: Network) -> Self {
        std::fs::create_dir_all(dir).expect("tailscale dir");
        let fake = FakeLocalApi::start(dir, Daemon::Running(network))
            .await
            .expect("fake tailscale");
        let api = LocalApi::new(fake.socket())
            .with_program_dirs(Vec::new())
            .with_timeout(porter_fake::GENEROUS);
        Self {
            fake,
            api,
            account: Arc::new(AtomicBool::new(true)),
        }
    }

    /// `org.quire.Tailnet1` as accountd serves it: this fake's computers, none when Tailscale
    /// is not an account.
    pub fn object(&self) -> FakeTailnet {
        FakeTailnet {
            api: self.api.clone(),
            account: Arc::clone(&self.account),
        }
    }

    /// The whois questions this Tailscale was asked: one per connection that arrived at a
    /// listener of this computer, since nothing else asks it.
    pub fn whois_asked(&self) -> usize {
        self.fake
            .requests()
            .iter()
            .filter(|line| line.contains("/localapi/v0/whois"))
            .count()
    }
}

/// The `org.quire.Tailnet1` object of the fake accountd.
#[derive(Debug)]
pub struct FakeTailnet {
    api: LocalApi,
    account: Arc<AtomicBool>,
}

#[zbus::interface(name = "org.quire.Tailnet1")]
impl FakeTailnet {
    async fn machines(&self) -> fdo::Result<Vec<(String, Details)>> {
        if !self.account.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        let Ok(status) = self.api.status().await else {
            return Ok(Vec::new());
        };
        Ok(status.machines().iter().map(machine_to_dbus).collect())
    }

    #[zbus(signal)]
    async fn changed(emitter: &zbus::object_server::SignalEmitter<'_>) -> zbus::Result<()>;
}

static NEXT: AtomicU8 = AtomicU8::new(0);

/// The addresses of one test: two computers' own, and two for others. In `127.x.y.0/24`, with x
/// and y from the process id, so two tests running at once (processes) never share one.
#[derive(Debug, Clone, Copy)]
pub struct Addresses {
    pub desk: IpAddr,
    pub pi: IpAddr,
    pub spare: IpAddr,
    pub other: IpAddr,
}

pub fn addresses() -> Addresses {
    let pid = std::process::id();
    let block = NEXT.fetch_add(1, Ordering::SeqCst).wrapping_mul(4);
    let at = |last: u8| {
        IpAddr::V4(Ipv4Addr::new(
            127,
            ((pid >> 8) & 0xff) as u8,
            (pid & 0xff) as u8,
            block.wrapping_add(last),
        ))
    };
    Addresses {
        desk: at(1),
        pi: at(2),
        spare: at(3),
        other: at(4),
    }
}

/// A port nothing is listening on now.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("a free port")
        .local_addr()
        .expect("its address")
        .port()
}

/// How the other computer is seen from a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    /// A computer of the same user.
    Mine,
    /// Tagged as a server.
    Tagged,
    /// Another person's.
    Another,
    /// Shared in by another person.
    Shared,
}

/// The network of the computer `me` (called `me_name`, user 1, at `me_at`), which sees the
/// other computer (`other_id`, `other_name`, at `other_at`) as `seen`.
pub fn network(
    (me_id, me_name, me_at): (&str, &str, IpAddr),
    (other_id, other_name, other_at): (&str, &str, IpAddr),
    seen: Seen,
) -> Network {
    let other = FakePeer::new(
        other_id,
        other_name,
        if seen == Seen::Another { 2 } else { 1 },
        &other_at.to_string(),
    );
    let other = match seen {
        Seen::Tagged => other.tagged("tag:ci"),
        Seen::Shared => other.shared_in(),
        Seen::Mine | Seen::Another => other,
    };
    Network::new(
        "ada@example.org",
        "ada@example.org",
        FakePeer::new(me_id, me_name, 1, &me_at.to_string()),
    )
    .with_user(2, "bob@example.net")
    .with_peers(vec![other])
}
