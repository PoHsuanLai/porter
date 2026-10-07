//! A fake Microsoft Graph drive: the part of `/v1.0/me/drive` a replica of OneDrive's app folder
//! uses (items by id and by path under `special/approot`, content with ranges and redirects,
//! simple and session uploads, delta, children, quota), behind `Authorization: Bearer`. Plain
//! HTTP on loopback. The bearer is what accountd's relay adds, so a test asserts that the app
//! side never sent one.

mod drive;
mod routes;

pub use drive::{CHUNK_UNIT, Drive};
pub use routes::{DEFAULT_MAIL, Knobs};

use crate::http::{Hit, Request, Response, serve};
use crate::net::{Bind, Listener};
use crate::oauth::IssuerHandle;
use crate::seen::{Running, Seen, lock};
use drive::Body;
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::ProviderSpec;
use routes::{Origins, State, answer, answer_link};
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};

/// Which bearers the drive accepts.
#[derive(Debug, Clone)]
pub(crate) enum Accepts {
    /// This one token.
    Fixed(String),
    /// Any access token this issuer minted and has not revoked.
    Issued(IssuerHandle),
}

impl Accepts {
    pub(crate) fn admits(&self, bearer: Option<&str>) -> bool {
        match (self, bearer) {
            (_, None) => false,
            (Accepts::Fixed(token), Some(presented)) => token == presented,
            (Accepts::Issued(issuer), Some(presented)) => issuer.access_is_live(presented),
        }
    }
}

#[derive(Debug, Clone)]
struct Shared {
    base: String,
    /// The origin the links point at, when it is not `base`.
    link: Option<String>,
    link_hits: Seen<Hit>,
    accepts: Accepts,
    state: Arc<Mutex<State>>,
    hits: Seen<Hit>,
}

/// The test's side of a running fake Graph drive.
#[derive(Debug, Clone)]
pub struct GraphHandle {
    shared: Shared,
}

/// The fake, bound and ready to serve.
#[derive(Debug)]
pub struct FakeGraph {
    listener: Listener,
    links: Option<Listener>,
    shared: Shared,
}

fn names_of(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

impl FakeGraph {
    /// Binds a drive that accepts `Authorization: Bearer <token>`, over plain HTTP on loopback.
    pub async fn bind(token: &str) -> io::Result<Self> {
        let listener = Listener::bind(&Bind::Loopback, "graph").await?;
        let port = crate::net::port_of(listener.address());
        Ok(Self {
            listener,
            links: None,
            shared: Shared {
                base: format!("http://127.0.0.1:{port}"),
                link: None,
                link_hits: Seen::default(),
                accepts: Accepts::Fixed(token.to_owned()),
                state: Arc::default(),
                hits: Seen::default(),
            },
        })
    }

    /// Binds a drive whose upload sessions and redirected downloads are on a SECOND loopback
    /// origin, as Graph's are on another host. That origin serves only those links and refuses
    /// (`401`) any request that carries an `Authorization` header; the drive's own origin does
    /// not serve them.
    pub async fn bind_linked(token: &str) -> io::Result<Self> {
        let mut fake = Self::bind(token).await?;
        let links = Listener::bind(&Bind::Loopback, "graph-links").await?;
        fake.shared.link = Some(format!(
            "http://127.0.0.1:{}",
            crate::net::port_of(links.address())
        ));
        fake.links = Some(links);
        Ok(fake)
    }

    /// Binds a drive (its links on its own origin, as [`FakeGraph::bind`]) that accepts
    /// every access token `issuer` has minted and not revoked, so a client that signs in to
    /// the fake issuer and refreshes is served; a token the issuer does not know is `401`.
    pub async fn bind_issued(issuer: &IssuerHandle) -> io::Result<Self> {
        let mut fake = Self::bind("").await?;
        fake.shared.accepts = Accepts::Issued(issuer.clone());
        Ok(fake)
    }

    /// [`FakeGraph::bind_issued`], serving on a task.
    pub async fn start_issued(issuer: &IssuerHandle) -> io::Result<Running<GraphHandle>> {
        let fake = Self::bind_issued(issuer).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }

    /// Like [`FakeGraph::bind_linked`], serving on a task.
    pub async fn start_linked(token: &str) -> io::Result<Running<GraphHandle>> {
        let fake = Self::bind_linked(token).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }

    /// The handle onto this fake.
    pub fn handle(&self) -> GraphHandle {
        GraphHandle {
            shared: self.shared.clone(),
        }
    }

    /// Binds and serves on a task.
    pub async fn start(token: &str) -> io::Result<Running<GraphHandle>> {
        let fake = Self::bind(token).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }
}

impl GraphHandle {
    /// `http://127.0.0.1:port`.
    pub fn base_url(&self) -> &str {
        &self.shared.base
    }

    /// `http://127.0.0.1:port` of the origin the links point at, when it is not the drive's own.
    pub fn link_url(&self) -> Option<&str> {
        self.shared.link.as_deref()
    }

    /// Every request the links' origin answered, oldest first.
    pub fn link_hits(&self) -> Vec<Hit> {
        self.shared.link_hits.all()
    }

    /// Creates or replaces the file at `path` below the app folder (folders on the way are made),
    /// as another device of the same account would.
    pub fn put_file(&self, path: &str, bytes: &[u8]) {
        let names = names_of(path);
        let mut state = lock(&self.shared.state);
        let Some((name, folders)) = names.split_last() else {
            return;
        };
        let approot = state.drive.approot().to_owned();
        let parent = state
            .drive
            .ensure(&approot, folders)
            .expect("the folders on the path are folders");
        state
            .drive
            .write(&parent, name, bytes.to_vec())
            .expect("the drive has room");
    }

    /// Deletes the item at `path` below the app folder.
    pub fn delete(&self, path: &str) {
        let mut state = lock(&self.shared.state);
        if let Some(id) = state.drive.at(&names_of(path)).map(|n| n.id.clone()) {
            state.drive.delete(&id);
        }
    }

    /// The bytes of the file at `path` below the app folder.
    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        let state = lock(&self.shared.state);
        match &state.drive.at(&names_of(path))?.body {
            Body::File(bytes) => Some(bytes.clone()),
            Body::Folder => None,
        }
    }

    /// The `eTag` of the item at `path` below the app folder.
    pub fn etag(&self, path: &str) -> Option<String> {
        lock(&self.shared.state)
            .drive
            .at(&names_of(path))
            .map(drive::Node::etag)
    }

    /// Every file below the app folder: its path and size, in path order.
    pub fn files(&self) -> Vec<(String, u64)> {
        let state = lock(&self.shared.state);
        let approot = state.drive.approot().to_owned();
        let mut found: Vec<(String, u64)> = state
            .drive
            .changes(&approot, 0, state.drive.seq())
            .iter()
            .filter(|n| matches!(n.body, Body::File(_)))
            .map(|n| (path_of(&state.drive, &n.id), n.size()))
            .collect();
        found.sort();
        found
    }

    /// Delta tokens handed out so far are no longer valid (`410 resyncRequired`).
    pub fn expire_delta_tokens(&self) {
        lock(&self.shared.state).drive.expire_tokens();
    }

    /// Gives the drive room for `total` bytes.
    pub fn set_limit(&self, total: u64) {
        lock(&self.shared.state).drive.set_limit(total);
    }

    /// Sets the address `GET /v1.0/me` names (the account's mail address).
    pub fn set_mail(&self, address: &str) {
        lock(&self.shared.state).mail = address.to_owned();
    }

    /// Changes how the server answers.
    pub fn set_knobs(&self, knobs: Knobs) {
        lock(&self.shared.state).knobs = knobs;
    }

    /// The next `count` requests with a bearer are answered `429` with this `Retry-After`.
    pub fn throttle(&self, count: u32, retry_after: u32) {
        lock(&self.shared.state).knobs.throttle = Some((count, retry_after));
    }

    /// Every request answered, oldest first.
    pub fn hits(&self) -> Vec<Hit> {
        self.shared.hits.all()
    }
}

/// The path of `id` below the app folder.
fn path_of(drive: &Drive, id: &str) -> String {
    let mut names = Vec::new();
    let mut at = drive.any(id);
    while let Some(node) = at {
        if node.id == drive.approot() {
            break;
        }
        names.push(node.name.clone());
        at = node.parent.as_deref().and_then(|p| drive.any(p));
    }
    names.reverse();
    names.join("/")
}

impl FakeServer for FakeGraph {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::Graph
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        crate::shipped::point(spec, porter_core::Family::Graph, &self.shared.base)
    }

    fn serve(self) -> impl Future<Output = ()> + Send {
        let shared = self.shared;
        let drive_state = Arc::clone(&shared.state);
        let own = shared.clone();
        let main = serve(
            self.listener,
            None,
            Arc::new(move |request: Request| {
                let response: Response = {
                    let mut state = lock(&own.state);
                    let origins = Origins {
                        base: &own.base,
                        link: own.link.as_deref().unwrap_or(&own.base),
                    };
                    answer(&mut state, origins, &own.accepts, &request)
                };
                own.hits.push(Hit::of(&request, &response));
                response
            }),
        );
        let links = self.links;
        async move {
            match links {
                None => main.await,
                Some(listener) => {
                    let side = serve(
                        listener,
                        None,
                        Arc::new(move |request: Request| {
                            let response = answer_link(&mut lock(&drive_state), true, &request);
                            shared.link_hits.push(Hit::of(&request, &response));
                            response
                        }),
                    );
                    tokio::join!(main, side);
                }
            }
        }
    }
}
