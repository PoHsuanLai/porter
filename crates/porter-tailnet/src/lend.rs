//! Listening for the person's other computers.
//!
//! [`lend`] listens, while it is switched on and Tailscale is running and signed in, on this
//! computer's network addresses and on no other: one listener per address Tailscale gave this
//! computer (and the configuration allows), never "any address". It follows them: an address
//! that goes away loses its listener and the connections on it, a new one gets one. A listener
//! that cannot be made (the address is not up yet, the port is taken for a moment) is tried
//! again with a growing pause, for as long as it is wanted.
//!
//! Every connection is judged before a byte of it is read: Tailscale is asked who has the
//! address it came from, and [`judge`] decides from the answer (see there). A refusal is
//! answered with the sentence the person reads. A connection that passes is handed to the
//! [`Visits`] handler with the computer it came from, and with a permit that counts it against
//! the limits: a computer may have a few connections at once, and this computer a few more in
//! all.

use crate::guests::Guests;
use crate::identity::Identity;
use crate::judge::{Welcome, judge};
use crate::observe::Seen;
use crate::refusal::Refusal;
use crate::{PORT, is_network_address};
use porter_core::NodeId;
use porter_tailscale::{LocalApi, TailscaleError};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;

/// How many connections are served at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// From one computer.
    pub per_guest: usize,
    /// From all of them.
    pub in_all: usize,
}

impl Default for Limits {
    /// Four from one computer (a model works on one thing at a time; a few more are waiting
    /// their turn), sixteen in all.
    fn default() -> Self {
        Self {
            per_guest: 4,
            in_all: 16,
        }
    }
}

/// How listening is set up.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// The port to listen on.
    pub port: u16,
    /// Which of the addresses Tailscale reports may be listened on.
    pub allowed: fn(IpAddr) -> bool,
    /// How many connections are served at once.
    pub limits: Limits,
    /// The pause before a listener that could not be made is tried again; it doubles with each
    /// failure up to [`Config::longest_pause`].
    pub first_pause: Duration,
    /// The longest pause between two tries.
    pub longest_pause: Duration,
}

impl Config {
    /// What a product uses: [`PORT`], the network's own addresses only, a try after a second
    /// and then after longer, up to half a minute.
    pub const fn product() -> Self {
        Self {
            port: PORT,
            allowed: is_network_address,
            limits: Limits {
                per_guest: 4,
                in_all: 16,
            },
            first_pause: Duration::from_secs(1),
            longest_pause: Duration::from_secs(30),
        }
    }
}

/// Where listening stands now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listening {
    /// The addresses listened on.
    pub bound: BTreeSet<SocketAddr>,
    /// The addresses that should be listened on and could not be yet.
    pub waiting: BTreeSet<IpAddr>,
}

/// A connection counted against the limits, until it is dropped.
#[derive(Debug)]
pub struct Permit {
    _all: OwnedSemaphorePermit,
    _one: OwnedSemaphorePermit,
}

/// A connection that passed the judgement. Its parts are private, so a handler holds it whole
/// (or takes the stream with the permit, [`Visit::into_stream`]): a handler that let the permit
/// go early would let more connections in than the limits say.
#[derive(Debug)]
pub struct Visit {
    stream: TcpStream,
    from: SocketAddr,
    welcome: Welcome,
    host: String,
    permit: Permit,
}

impl Visit {
    /// Where it came from.
    pub fn from(&self) -> SocketAddr {
        self.from
    }

    /// The computer it came from, and where it stands with the person.
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    /// What this computer is called on the network.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The connection, nothing of it read, with the permit that counts it against the limits:
    /// the connection counts until the permit is dropped.
    pub fn into_stream(self) -> (TcpStream, Permit) {
        (self.stream, self.permit)
    }
}

/// What serves the connections that pass.
pub trait Visits: Send + Sync + 'static {
    /// Serves one connection to its end.
    fn visit(&self, visit: Visit) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// The listeners of one computer; they stop when this is dropped.
#[derive(Debug)]
pub struct Lending {
    listening: watch::Receiver<Listening>,
    task: JoinHandle<()>,
}

impl Lending {
    /// Where listening stands; the receiver changes when it does.
    pub fn listening(&self) -> watch::Receiver<Listening> {
        self.listening.clone()
    }
}

impl Drop for Lending {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Limiter {
    all: Arc<Semaphore>,
    per_guest: usize,
    each: Mutex<BTreeMap<NodeId, Arc<Semaphore>>>,
}

impl Limiter {
    fn new(limits: Limits) -> Self {
        Self {
            all: Arc::new(Semaphore::new(limits.in_all)),
            per_guest: limits.per_guest,
            each: Mutex::default(),
        }
    }

    fn take(&self, node: &NodeId) -> Option<Permit> {
        let all = Arc::clone(&self.all).try_acquire_owned().ok()?;
        let each = Arc::clone(
            self.each
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(node.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(self.per_guest))),
        );
        Some(Permit {
            _all: all,
            _one: each.try_acquire_owned().ok()?,
        })
    }
}

struct Shared<V> {
    api: LocalApi,
    seen: watch::Receiver<Seen>,
    guests: Arc<Guests>,
    visits: Arc<V>,
    limiter: Limiter,
}

/// Starts listening whenever `on` says so and `seen` says Tailscale is running and signed in.
/// Every connection is judged against `guests` and Tailscale (`api`) and, if it passes, served
/// by `visits`.
pub fn lend<V: Visits>(
    api: LocalApi,
    seen: watch::Receiver<Seen>,
    on: watch::Receiver<bool>,
    guests: Arc<Guests>,
    visits: Arc<V>,
    config: Config,
) -> Lending {
    let (tx, listening) = watch::channel(Listening::default());
    let shared = Arc::new(Shared {
        api,
        seen: seen.clone(),
        guests,
        visits,
        limiter: Limiter::new(config.limits),
    });
    let task = tokio::spawn(run(shared, seen, on, config, tx));
    Lending { listening, task }
}

/// The addresses that should be listened on now.
fn wanted(
    on: &watch::Receiver<bool>,
    seen: &watch::Receiver<Seen>,
    config: &Config,
) -> BTreeSet<IpAddr> {
    if !*on.borrow() {
        return BTreeSet::new();
    }
    match &*seen.borrow() {
        Some(me) => me
            .addresses()
            .iter()
            .copied()
            .filter(|ip| !ip.is_unspecified() && (config.allowed)(*ip))
            .collect(),
        None => BTreeSet::new(),
    }
}

struct Retry {
    at: Instant,
    pause: Duration,
}

async fn until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn run<V: Visits>(
    shared: Arc<Shared<V>>,
    mut seen: watch::Receiver<Seen>,
    mut on: watch::Receiver<bool>,
    config: Config,
    tx: watch::Sender<Listening>,
) {
    // Dropped with this task (when `Lending` is), which ends every listener with it.
    let mut live: BTreeMap<IpAddr, Guard> = BTreeMap::new();
    let mut failing: BTreeMap<IpAddr, Retry> = BTreeMap::new();
    loop {
        let want = wanted(&on, &seen, &config);
        live.retain(|ip, _| want.contains(ip));
        failing.retain(|ip, _| want.contains(ip));
        let now = Instant::now();
        for ip in &want {
            if live.contains_key(ip) || failing.get(ip).is_some_and(|retry| retry.at > now) {
                continue;
            }
            match TcpListener::bind(SocketAddr::new(*ip, config.port)).await {
                Ok(listener) => {
                    failing.remove(ip);
                    live.insert(
                        *ip,
                        Guard(tokio::spawn(accept(listener, Arc::clone(&shared)))),
                    );
                }
                Err(_) => {
                    let pause = match failing.get(ip) {
                        Some(retry) => (retry.pause * 2).min(config.longest_pause),
                        None => config.first_pause,
                    };
                    failing.insert(
                        *ip,
                        Retry {
                            at: now + pause,
                            pause,
                        },
                    );
                }
            }
        }
        let listening = Listening {
            bound: live
                .keys()
                .map(|ip| SocketAddr::new(*ip, config.port))
                .collect(),
            waiting: failing.keys().copied().collect(),
        };
        tx.send_if_modified(|now| {
            let changed = *now != listening;
            if changed {
                *now = listening;
            }
            changed
        });
        let next = failing.values().map(|retry| retry.at).min();
        tokio::select! {
            changed = on.changed() => if changed.is_err() { break },
            changed = seen.changed() => if changed.is_err() { break },
            () = until(next) => {}
        }
    }
}

/// A task that ends when this is dropped.
struct Guard(JoinHandle<()>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Accepts connections on one listener until it is aborted; its connections end with it.
async fn accept<V: Visits>(listener: TcpListener, shared: Arc<Shared<V>>) {
    let mut connections = JoinSet::new();
    let mut pause = Duration::from_millis(50);
    loop {
        match listener.accept().await {
            Ok((stream, from)) => {
                pause = Duration::from_millis(50);
                connections.spawn(connection(stream, from, Arc::clone(&shared)));
                while connections.try_join_next().is_some() {}
            }
            // Out of descriptors, or a connection that reset before it was taken: a passing
            // trouble. Wait a little, longer each time, and go on.
            Err(_) => {
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(Duration::from_secs(2));
            }
        }
    }
}

/// What a refusal is answered with: a JSON error in the shape model clients read.
fn refusal_response(refusal: &Refusal) -> String {
    let status = refusal.status();
    let reason = match status {
        403 => "Forbidden",
        429 => "Too Many Requests",
        _ => "Service Unavailable",
    };
    let body = serde_json::json!({
        "error": {"message": refusal.to_string(), "type": "refused", "code": "refused"}
    })
    .to_string();
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Answers a refused connection with the sentence, after taking what the other side has sent
/// first (so that the answer is not lost to a reset), and closes it. A computer that sends
/// nothing is not waited for for ever.
async fn refuse(mut stream: TcpStream, refusal: Refusal) {
    let mut first = [0_u8; 4096];
    let _ = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut first)).await;
    let _ = stream
        .write_all(refusal_response(&refusal).as_bytes())
        .await;
    let _ = stream.shutdown().await;
}

async fn connection<V: Visits>(stream: TcpStream, from: SocketAddr, shared: Arc<Shared<V>>) {
    let from = SocketAddr::new(from.ip().to_canonical(), from.port());
    let Some(me) = shared.seen.borrow().clone() else {
        return refuse(stream, Refusal::Off).await;
    };
    match admit(&shared, &me, from).await {
        Ok(welcome) => match shared.limiter.take(&welcome.peer().node) {
            Some(permit) => {
                let host = me.name().to_owned();
                shared
                    .visits
                    .visit(Visit {
                        stream,
                        from,
                        welcome,
                        host,
                        permit,
                    })
                    .await;
            }
            None => refuse(stream, Refusal::Busy).await,
        },
        Err(refusal) => refuse(stream, refusal).await,
    }
}

/// Asks Tailscale who has `from` (not when it is this computer's own address: that is refused
/// without asking) and judges the answer.
async fn admit<V>(shared: &Shared<V>, me: &Identity, from: SocketAddr) -> Result<Welcome, Refusal> {
    let answer = if me.owns(from.ip()) {
        Err(TailscaleError::NoSuchPeer)
    } else {
        shared.api.whois(from).await
    };
    judge(me, from.ip(), answer, &shared.guests)
}
