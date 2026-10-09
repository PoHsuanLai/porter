//! A fake Tailscale LocalAPI: the HTTP tailscaled serves on its unix socket, in a scratch
//! directory. It answers `status`, `whois`, `login-interactive` and the `watch-ipn-bus` stream
//! from a table the test sets, with the checks the real one makes (the `Host` it expects, no
//! `Origin`, the stream without private keys), and it can be stopped (the socket stays, nobody
//! answers), refusing, signed out, in the middle of changing, or read-only. It never touches the
//! real tailscaled or `/var/run/tailscale`.
//!
//! The JSON follows Tailscale v1.80.0 (`ipn/ipnstate`, `tailcfg`, `ipn`): Go's own field names.

use crate::http::{Request, Response, read_request, write_response};
use crate::seen::{Seen, lock};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Go's zero time, which is how Tailscale says "never".
const NEVER: &str = "0001-01-01T00:00:00Z";

/// A user of the fake network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeUser {
    /// Its id.
    pub id: i64,
    /// Its login.
    pub login: String,
}

/// One computer of the fake network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakePeer {
    /// The stable node id (`nABC123CNTRL`).
    pub id: String,
    /// The name (the first label of its network name).
    pub name: String,
    /// The user that made it.
    pub user: i64,
    /// Its operating system.
    pub os: String,
    /// Whether it is connected.
    pub online: bool,
    /// When it was last connected (RFC 3339), for an offline one.
    pub last_seen: Option<String>,
    /// Its tags.
    pub tags: Vec<String>,
    /// Whether it was shared in from another person.
    pub shared: bool,
    /// The SSH host keys it advertises.
    pub ssh_host_keys: Vec<String>,
    /// Its addresses.
    pub addresses: Vec<String>,
}

impl FakePeer {
    /// An online Linux computer of user `user`.
    pub fn new(id: &str, name: &str, user: i64, address: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            user,
            os: "linux".to_owned(),
            online: true,
            last_seen: None,
            tags: Vec::new(),
            shared: false,
            ssh_host_keys: Vec::new(),
            addresses: vec![address.to_owned()],
        }
    }

    /// Offline since `last_seen`.
    pub fn offline(mut self, last_seen: &str) -> Self {
        self.online = false;
        self.last_seen = Some(last_seen.to_owned());
        self
    }

    /// With this tag.
    pub fn tagged(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_owned());
        self
    }

    /// Shared in from another person.
    pub fn shared_in(mut self) -> Self {
        self.shared = true;
        self
    }

    /// With Tailscale SSH on (one host key).
    pub fn with_ssh(mut self) -> Self {
        self.ssh_host_keys
            .push(format!("ssh-ed25519 AAAA{}", self.name));
        self
    }

    fn dns(&self, tailnet_suffix: &str) -> String {
        format!("{}.{tailnet_suffix}.", self.name)
    }

    fn status_json(&self, suffix: &str) -> Value {
        let mut node = json!({
            "ID": self.id, "HostName": self.name, "DNSName": self.dns(suffix), "OS": self.os,
            "UserID": self.user, "TailscaleIPs": self.addresses, "Online": self.online,
            "LastSeen": self.last_seen.as_deref().unwrap_or(NEVER),
        });
        if !self.tags.is_empty() {
            node["Tags"] = json!(self.tags);
        }
        if self.shared {
            node["ShareeNode"] = json!(true);
        }
        if !self.ssh_host_keys.is_empty() {
            node["sshHostKeys"] = json!(self.ssh_host_keys);
        }
        node
    }

    fn whois_json(&self, suffix: &str) -> Value {
        let mut node = json!({
            "ID": 1, "StableID": self.id, "Name": self.dns(suffix), "User": self.user,
            "Addresses": self.addresses.iter().map(|a| format!("{a}/32")).collect::<Vec<_>>(),
            "Online": self.online,
        });
        if !self.tags.is_empty() {
            node["Tags"] = json!(self.tags);
        }
        if self.shared {
            node["Sharer"] = json!(self.user + 1000);
        }
        node
    }
}

/// A signed-in network: this computer, the others and the users.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    /// The network's name.
    pub name: String,
    /// The suffix of its computers' names (`tail1234.ts.net`).
    pub suffix: String,
    /// This computer.
    pub me: FakePeer,
    /// The others.
    pub peers: Vec<FakePeer>,
    /// The users.
    pub users: Vec<FakeUser>,
}

impl Network {
    /// A network `name` with this computer, signed in as `login` (user 1).
    pub fn new(name: &str, login: &str, me: FakePeer) -> Self {
        Self {
            name: name.to_owned(),
            suffix: "tail1234.ts.net".to_owned(),
            me,
            peers: Vec::new(),
            users: vec![FakeUser {
                id: 1,
                login: login.to_owned(),
            }],
        }
    }

    /// With these other computers.
    pub fn with_peers(mut self, peers: Vec<FakePeer>) -> Self {
        self.peers = peers;
        self
    }

    /// With one more user.
    pub fn with_user(mut self, id: i64, login: &str) -> Self {
        self.users.push(FakeUser {
            id,
            login: login.to_owned(),
        });
        self
    }

    fn user_json(&self, id: i64) -> Value {
        let login = self
            .users
            .iter()
            .find(|u| u.id == id)
            .map_or("unknown@example.org", |u| u.login.as_str());
        json!({"ID": id, "LoginName": login, "DisplayName": login})
    }

    fn find(&self, address: &str) -> Option<&FakePeer> {
        std::iter::once(&self.me)
            .chain(&self.peers)
            .find(|p| p.addresses.iter().any(|a| a == address))
    }
}

/// What the fake daemon is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
// A test builds a handful of these and writes `Daemon::Running(network)`; boxing the network
// would put a `Box::new` in every one of them for no gain.
#[allow(clippy::large_enum_variant)]
pub enum Daemon {
    /// Running, nobody signed in. `auth_url` is the page Tailscale gives once a sign-in is
    /// asked for (`login-interactive`); until then `status` has none.
    SignedOut {
        /// Tailscale's sign-in page.
        auth_url: String,
    },
    /// Starting: it says so and nothing else.
    Changing,
    /// Running and signed in.
    Running(Network),
    /// Answering every request `403`: the socket is not open to this user.
    Refusing,
}

impl Daemon {
    fn state_number(&self) -> u8 {
        match self {
            Daemon::SignedOut { .. } => 2,
            Daemon::Changing => 5,
            Daemon::Running(_) | Daemon::Refusing => 6,
        }
    }
}

#[derive(Debug)]
struct Inner {
    daemon: Daemon,
    /// `login-interactive` is refused `403` (the user may read but is not the operator).
    read_only: bool,
    /// A sign-in was asked for since the daemon was last signed in.
    login_asked: bool,
}

#[derive(Debug)]
struct Shared {
    inner: Mutex<Inner>,
    news: broadcast::Sender<String>,
    seen: Seen<String>,
    watchers: AtomicUsize,
    logins: AtomicUsize,
}

impl Shared {
    fn say(&self, line: Value) {
        // Nobody listening is fine: the stream is for whoever watches.
        let _ = self.news.send(format!("{line}\n"));
    }

    fn state_line(daemon: &Daemon) -> Value {
        json!({"Version": "1.80.0-fake", "State": daemon.state_number()})
    }

    fn status(&self) -> Value {
        let inner = lock(&self.inner);
        match &inner.daemon {
            Daemon::Running(net) => {
                let peers: BTreeMap<String, Value> = net
                    .peers
                    .iter()
                    .map(|p| (format!("nodekey:{}", p.id), p.status_json(&net.suffix)))
                    .collect();
                let users: BTreeMap<String, Value> = net
                    .users
                    .iter()
                    .map(|u| (u.id.to_string(), net.user_json(u.id)))
                    .collect();
                json!({
                    "Version": "1.80.0-fake", "BackendState": "Running", "AuthURL": "",
                    "Self": net.me.status_json(&net.suffix),
                    "CurrentTailnet": {"Name": net.name, "MagicDNSSuffix": net.suffix, "MagicDNSEnabled": true},
                    "Peer": peers, "User": users,
                })
            }
            Daemon::SignedOut { auth_url } => json!({
                "Version": "1.80.0-fake", "BackendState": "NeedsLogin",
                "AuthURL": if inner.login_asked { auth_url.as_str() } else { "" },
                "Peer": null, "User": null,
            }),
            Daemon::Changing | Daemon::Refusing => json!({
                "Version": "1.80.0-fake", "BackendState": "Starting", "AuthURL": "",
            }),
        }
    }

    fn whois(&self, address: &str) -> Response {
        let inner = lock(&self.inner);
        let Daemon::Running(net) = &inner.daemon else {
            return Response::new(500).typed("text/plain", "no netmap");
        };
        // The query holds an address, with the port when the caller has one.
        let ip = match address.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => {
                host.trim_matches(['[', ']']).to_owned()
            }
            _ => address.to_owned(),
        };
        match net.find(&ip) {
            Some(peer) => Response::json(
                200,
                &json!({
                    "Node": peer.whois_json(&net.suffix),
                    "UserProfile": net.user_json(peer.user),
                    "CapMap": {},
                }),
            ),
            None => Response::new(404).typed("text/plain", format!("no match for {address}")),
        }
    }
}

/// The test's side of a fake LocalAPI.
#[derive(Debug, Clone)]
pub struct LocalApiHandle {
    shared: Arc<Shared>,
    socket: PathBuf,
}

/// A fake Tailscale LocalAPI, serving until it is stopped or dropped.
#[derive(Debug)]
pub struct FakeLocalApi {
    handle: LocalApiHandle,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl FakeLocalApi {
    /// Serves on `dir/tailscaled.sock` (a scratch directory), starting as `daemon`.
    pub async fn start(dir: &Path, daemon: Daemon) -> io::Result<Self> {
        let (news, _) = broadcast::channel(256);
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                daemon,
                read_only: false,
                login_asked: false,
            }),
            news,
            seen: Seen::default(),
            watchers: AtomicUsize::new(0),
            logins: AtomicUsize::new(0),
        });
        let socket = dir.join("tailscaled.sock");
        let task = tokio::spawn(serve(UnixListener::bind(&socket)?, Arc::clone(&shared)));
        Ok(Self {
            handle: LocalApiHandle { shared, socket },
            task: Mutex::new(Some(task)),
        })
    }

    /// The handle onto it.
    pub fn handle(&self) -> LocalApiHandle {
        self.handle.clone()
    }

    /// Stops answering: the socket file stays and nobody listens on it, as after tailscaled
    /// was killed (a connection is refused, and a watcher's stream ends).
    pub fn stop(&self) {
        if let Some(task) = lock(&self.task).take() {
            task.abort();
        }
    }

    /// Answers again, on the same path.
    pub async fn restart(&self) -> io::Result<()> {
        self.stop();
        // A stale socket file is what a killed daemon leaves; the new one replaces it.
        let _ = std::fs::remove_file(&self.handle.socket);
        let listener = UnixListener::bind(&self.handle.socket)?;
        *lock(&self.task) = Some(tokio::spawn(serve(
            listener,
            Arc::clone(&self.handle.shared),
        )));
        Ok(())
    }
}

impl std::ops::Deref for FakeLocalApi {
    type Target = LocalApiHandle;

    fn deref(&self) -> &LocalApiHandle {
        &self.handle
    }
}

impl Drop for FakeLocalApi {
    fn drop(&mut self) {
        self.stop();
    }
}

impl LocalApiHandle {
    /// The socket path to give the code under test.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Becomes `daemon`, and tells whoever watches.
    pub fn set(&self, daemon: Daemon) {
        let line = Shared::state_line(&daemon);
        let running = matches!(daemon, Daemon::Running(_));
        {
            let mut inner = lock(&self.shared.inner);
            if running {
                inner.login_asked = false;
            }
            inner.daemon = daemon;
        }
        self.shared.say(line);
        if running {
            self.shared.say(json!({"NetMap": {}}));
        }
    }

    /// Changes the network (a computer comes or goes, goes offline), and tells whoever watches.
    /// Does nothing unless the daemon is running.
    pub fn edit(&self, change: impl FnOnce(&mut Network)) {
        let changed = match &mut lock(&self.shared.inner).daemon {
            Daemon::Running(net) => {
                change(net);
                true
            }
            _ => false,
        };
        if changed {
            self.shared.say(json!({"NetMap": {}}));
        }
    }

    /// Changes the network without telling anyone who watches (a change the stream missed).
    pub fn edit_quietly(&self, change: impl FnOnce(&mut Network)) {
        if let Daemon::Running(net) = &mut lock(&self.shared.inner).daemon {
            change(net);
        }
    }

    /// Whether `login-interactive` is refused (the user may read the socket but is not its
    /// operator).
    pub fn set_read_only(&self, read_only: bool) {
        lock(&self.shared.inner).read_only = read_only;
    }

    /// Every request received, `METHOD target`, oldest first.
    pub fn requests(&self) -> Vec<String> {
        self.shared.seen.all()
    }

    /// How many watch streams are open now.
    pub fn watchers(&self) -> usize {
        self.shared.watchers.load(Ordering::SeqCst)
    }

    /// How many sign-ins were asked for.
    pub fn logins(&self) -> usize {
        self.shared.logins.load(Ordering::SeqCst)
    }
}

async fn serve(listener: UnixListener, shared: Arc<Shared>) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { return };
                connections.spawn(connection(stream, Arc::clone(&shared)));
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn connection(stream: UnixStream, shared: Arc<Shared>) {
    let mut reader = BufReader::new(stream);
    while let Ok(Some(request)) = read_request(&mut reader).await {
        shared
            .seen
            .push(format!("{} {}", request.method, request.target));
        match answer(&shared, &request) {
            Answer::Plain(response) => {
                if write_response(reader.get_mut(), &response, false)
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Answer::Watch => {
                // The stream owns the connection until either side ends it.
                let _ = watch(reader.get_mut(), &shared).await;
                return;
            }
        }
    }
}

enum Answer {
    Plain(Response),
    Watch,
}

fn answer(shared: &Shared, request: &Request) -> Answer {
    // What the real server refuses before it looks at the path.
    let host_ok = request
        .header("host")
        .is_none_or(|host| host == "local-tailscaled.sock");
    if !host_ok || request.header("origin").is_some() || request.header("referer").is_some() {
        return Answer::Plain(Response::new(403).typed("text/plain", "invalid localapi request"));
    }
    let (refusing, read_only) = {
        let inner = lock(&shared.inner);
        (matches!(inner.daemon, Daemon::Refusing), inner.read_only)
    };
    if refusing {
        return Answer::Plain(Response::new(403).typed("text/plain", "access denied"));
    }
    let plain = |response| Answer::Plain(response);
    match (request.method.as_str(), request.path()) {
        ("GET", "/localapi/v0/status") => plain(Response::json(200, &shared.status())),
        ("GET", "/localapi/v0/whois") => match request.query_value("addr") {
            Some(addr) => plain(shared.whois(&addr)),
            None => plain(Response::new(400).typed("text/plain", "missing 'addr' parameter")),
        },
        ("POST", "/localapi/v0/login-interactive") => match read_only {
            true => plain(Response::new(403).typed("text/plain", "login access denied")),
            false => {
                shared.logins.fetch_add(1, Ordering::SeqCst);
                let url = {
                    let mut inner = lock(&shared.inner);
                    inner.login_asked = true;
                    match &inner.daemon {
                        Daemon::SignedOut { auth_url } => Some(auth_url.clone()),
                        _ => None,
                    }
                };
                if let Some(url) = url {
                    shared.say(json!({"BrowseToURL": url}));
                }
                plain(Response::new(204))
            }
        },
        ("GET", "/localapi/v0/watch-ipn-bus") => {
            // Without NotifyNoPrivateKeys (16) the real stream wants write access.
            let mask: u32 = request
                .query_value("mask")
                .and_then(|m| m.parse().ok())
                .unwrap_or(0);
            match mask & 16 != 0 || !read_only {
                true => Answer::Watch,
                false => plain(Response::new(403).typed("text/plain", "watch access denied")),
            }
        }
        ("GET" | "POST", _) => plain(Response::new(404).typed("text/plain", "404 page not found")),
        _ => plain(Response::new(405).typed("text/plain", "method not allowed")),
    }
}

/// Counts an open stream while it lives.
struct Watching<'a>(&'a AtomicUsize);

impl<'a> Watching<'a> {
    fn begin(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}

impl Drop for Watching<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn chunk(stream: &mut UnixStream, line: &str) -> io::Result<()> {
    stream
        .write_all(format!("{:x}\r\n{line}\r\n", line.len()).as_bytes())
        .await?;
    stream.flush().await
}

/// `watch-ipn-bus`: a chunked body of one notice per line, the state first, then what changes.
async fn watch(stream: &mut UnixStream, shared: &Shared) -> io::Result<()> {
    // Subscribe before the first line, so a change right after it is not missed.
    let mut news = shared.news.subscribe();
    let _open = Watching::begin(&shared.watchers);
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n",
        )
        .await?;
    let first = {
        let inner = lock(&shared.inner);
        let mut lines = format!("{}\n", Shared::state_line(&inner.daemon));
        if matches!(inner.daemon, Daemon::Running(_)) {
            lines.push_str("{\"NetMap\":{}}\n");
        }
        lines
    };
    for line in first.lines() {
        chunk(stream, &format!("{line}\n")).await?;
    }
    let mut ignored = [0u8; 64];
    loop {
        tokio::select! {
            // The client sends nothing more; a read that returns is the client leaving.
            _ = stream.read(&mut ignored) => return Ok(()),
            received = news.recv() => match received {
                Ok(line) => chunk(stream, &line).await?,
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
        }
    }
}
