//! What the tests share: a pair of addresses of their own, a fake Tailscale, and waits.

use porter_fake::GENEROUS;
use porter_fake_servers::{Daemon, FakeLocalApi, FakePeer, Network};
use porter_tailnet::{Config, Limits, Observer, Seen, Timing};
use porter_tailscale::LocalApi;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::watch;

/// A new empty directory for one test, under the test's temporary directory.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("porter-tailnet-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

static NEXT: AtomicU8 = AtomicU8::new(0);

/// Addresses of this test alone: this computer's and three others', in `127.x.y.0/24` with x and
/// y from the process id, so two tests running at once (processes) never share one. They are
/// loopback addresses: nothing leaves this computer, and a socket bound to one is reached from a
/// socket bound to another.
pub struct Addresses {
    pub me: IpAddr,
    pub pi: IpAddr,
    pub other: IpAddr,
    pub spare: IpAddr,
}

pub fn addresses() -> Addresses {
    let pid = std::process::id();
    // One block of four per call; a process makes far fewer than 60 calls.
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
        me: at(1),
        pi: at(2),
        other: at(3),
        spare: at(4),
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

/// This computer (`desk`, user 1) and four others: `pi` (user 1), `build-box` (tagged),
/// `friends-pc` (user 2) and `old-laptop` (user 1, offline), the last three at addresses that
/// need not exist.
pub fn network(a: &Addresses) -> Network {
    Network::new(
        "ada@example.org",
        "ada@example.org",
        FakePeer::new("nSELF", "desk", 1, &a.me.to_string()),
    )
    .with_user(2, "bob@example.net")
    .with_peers(vec![
        FakePeer::new("nPI", "pi", 1, &a.pi.to_string()),
        FakePeer::new("nBUILD", "build-box", 1, &a.other.to_string()).tagged("tag:ci"),
        FakePeer::new("nFRIEND", "friends-pc", 2, &a.spare.to_string()).shared_in(),
        FakePeer::new("nOLD", "old-laptop", 1, "127.9.9.9").offline("2026-09-21T14:13:20Z"),
    ])
}

/// A fake Tailscale running `daemon`, and a client of it that finds no Tailscale program.
pub async fn fake(name: &str, daemon: Daemon) -> (FakeLocalApi, LocalApi) {
    let dir = scratch(name);
    let fake = FakeLocalApi::start(&dir, daemon).await.expect("fake");
    let api = LocalApi::new(fake.socket())
        .with_program_dirs(Vec::new())
        .with_timeout(Duration::from_secs(30));
    (fake, api)
}

/// The timing tests follow Tailscale at: quick, since a test is not a person's computer.
pub fn quick() -> Timing {
    Timing {
        backstop: Duration::from_millis(300),
        retry: Duration::from_millis(100),
    }
}

/// Listening as a test sets it up: its own port, any address that is not plain loopback.
pub fn config(port: u16) -> Config {
    Config {
        port,
        allowed: |ip| ip.is_loopback() && ip != IpAddr::V4(Ipv4Addr::LOCALHOST),
        limits: Limits::default(),
        first_pause: Duration::from_millis(50),
        longest_pause: Duration::from_millis(200),
    }
}

/// Follows `api` and waits until this computer is known.
pub async fn observed(api: &LocalApi) -> (Observer, watch::Receiver<Seen>) {
    let observer = Observer::start(api.clone(), quick());
    let mut seen = observer.seen();
    wait(&mut seen, |now| now.is_some(), "this computer to be seen").await;
    (observer, seen)
}

/// Waits until `condition` holds of the value `rx` carries; fails the test if it does not within
/// the generous wait.
pub async fn wait<T>(rx: &mut watch::Receiver<T>, condition: impl FnMut(&T) -> bool, what: &str) {
    match tokio::time::timeout(GENEROUS, rx.wait_for(condition)).await {
        Ok(Ok(_)) => {}
        _ => panic!("nothing like {what} within {GENEROUS:?}"),
    }
}

/// A connection from the address `source` to `to`.
pub async fn connect_from(source: IpAddr, to: SocketAddr) -> std::io::Result<TcpStream> {
    let socket = TcpSocket::new_v4()?;
    socket.bind(SocketAddr::new(source, 0))?;
    socket.connect(to).await
}

/// Sends a small request and reads the answer to its end.
pub async fn ask(mut stream: TcpStream, request: &str) -> String {
    stream.write_all(request.as_bytes()).await.expect("send");
    let mut answer = Vec::new();
    tokio::time::timeout(GENEROUS, stream.read_to_end(&mut answer))
        .await
        .expect("an answer within the generous wait")
        .ok();
    String::from_utf8_lossy(&answer).into_owned()
}

/// Whether anything is listening at `to` (a connection is made from the loopback address).
pub async fn listening_at(to: SocketAddr) -> bool {
    TcpStream::connect(to).await.is_ok()
}
