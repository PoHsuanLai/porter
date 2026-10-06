//! The rig: a fake Graph drive on loopback, reached through `StreamHttp` over a TCP dial with
//! the bearer added the way accountd's relay would add it. No network beyond loopback.
#![allow(dead_code)]

use porter_core::stream::ByteStream;
use porter_core::{UnixSeconds, WebUrl};
use porter_fake_servers::{FakeGraph, GraphHandle, Running};
use porter_http::{Header, Http, HttpError, HttpRequest, HttpResponse};
use std::future::Future;
use std::io;
use storage_graph::{Clock, Dial, GraphReplica, StreamHttp, StreamLimits, Uploads};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const TOKEN: &str = "ey.fake-access-token";
pub const NOW: UnixSeconds = UnixSeconds(1_000);

/// A TCP stream as the byte stream a relay's descriptor would be.
#[derive(Debug)]
pub struct Tcp(TcpStream);

impl ByteStream for Tcp {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf).await
    }

    async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes).await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.0.shutdown().await
    }
}

/// Dials the fake's loopback port.
#[derive(Debug)]
pub struct TcpDial(pub u16);

impl Dial for TcpDial {
    type Stream = Tcp;

    async fn dial(&self) -> Result<Tcp, HttpError> {
        TcpStream::connect(("127.0.0.1", self.0))
            .await
            .map(Tcp)
            .map_err(|_| HttpError::Unreachable)
    }
}

/// Adds the bearer to every request, as the relay does on the far side.
#[derive(Debug)]
pub struct Bearer<H>(pub H);

impl<H: Http> Http for Bearer<H> {
    fn send(
        &self,
        mut request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> + Send {
        request
            .headers
            .push(Header::new("Authorization", format!("Bearer {TOKEN}")));
        self.0.send(request)
    }
}

pub type Client = Bearer<StreamHttp<TcpDial>>;
pub type Replica = GraphReplica<Client>;

pub fn client(port: u16) -> Client {
    Bearer(StreamHttp::new(TcpDial(port), StreamLimits::default()))
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
        GraphReplica::new(client(self.port), &self.base, folder, Clock::fixed(NOW))
            .with_page(page)
            .with_uploads(uploads)
    }
}
