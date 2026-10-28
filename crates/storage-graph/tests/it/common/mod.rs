//! The rig: a fake Graph drive on loopback, reached through `RelayHttp` over a TCP dial with
//! the bearer added the way accountd's relay would add it. No network beyond loopback.
#![allow(dead_code)]

use porter_fake_servers::{FakeGraph, GraphHandle, Running};
use porter_http::http::header::{AUTHORIZATION, HeaderValue};
use porter_http::http::{Request, Response};
use porter_http::{Http, HttpError};
use porter_http_relay::{Dial, RelayHttp, RelayLimits};
use porter_ids::{UnixSeconds, WebUrl};
use porter_sync::Clock;
use std::future::Future;
use storage_graph::{GraphReplica, Root, Uploads};
use tokio::net::TcpStream;

pub const TOKEN: &str = "ey.fake-access-token";
pub const NOW: UnixSeconds = UnixSeconds(1_000);

/// Dials the fake's loopback port.
#[derive(Debug)]
pub struct TcpDial(pub u16);

impl Dial for TcpDial {
    type Stream = TcpStream;

    async fn dial(&self) -> Result<TcpStream, HttpError> {
        TcpStream::connect(("127.0.0.1", self.0))
            .await
            .map_err(|_| HttpError::Unreachable)
    }
}

/// Adds the bearer to every request, as the relay does on the far side.
#[derive(Debug)]
pub struct Bearer<H>(pub H);

impl<H: Http> Http for Bearer<H> {
    fn send(
        &self,
        mut request: Request<Vec<u8>>,
    ) -> impl Future<Output = Result<Response<Vec<u8>>, HttpError>> + Send {
        let value = HeaderValue::try_from(format!("Bearer {TOKEN}")).expect("a header value");
        request.headers_mut().append(AUTHORIZATION, value);
        self.0.send(request)
    }
}

pub type Client = Bearer<RelayHttp<TcpDial>>;
pub type Replica = GraphReplica<Client>;

pub fn client(port: u16) -> Client {
    Bearer(RelayHttp::new(TcpDial(port), RelayLimits::default()))
}

/// A running fake Graph drive.
pub struct Server {
    pub graph: Running<GraphHandle>,
    pub port: u16,
    pub base: WebUrl,
}

impl Server {
    pub async fn start() -> Self {
        let graph = FakeGraph::start(TOKEN).await.expect("fake graph");
        let port: u16 = graph
            .base_url()
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .expect("a port");
        let base = WebUrl::parse(graph.base_url()).expect("url");
        Self { graph, port, base }
    }

    /// A replica of `folder` below the app folder with delta pages of `page` items and these
    /// upload limits; each call is a new replica (a new process, as far as caches go).
    pub fn replica(&self, folder: &str, page: usize, uploads: Uploads) -> Replica {
        self.replica_at(Root::AppFolder, folder, page, uploads)
    }

    /// The same, of `folder` below `root`.
    pub fn replica_at(&self, root: Root, folder: &str, page: usize, uploads: Uploads) -> Replica {
        GraphReplica::new(client(self.port), &self.base, folder, Clock::fixed(NOW))
            .with_root(root)
            .with_page(page)
            .with_uploads(uploads)
    }
}
