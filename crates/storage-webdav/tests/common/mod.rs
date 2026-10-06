//! The rig: a fake DAV server on loopback, reached through `StreamHttp` over a TCP dial with
//! the credential added the way accountd's relay would add it. No network beyond loopback.
#![allow(dead_code)]

use porter_core::stream::ByteStream;
use porter_core::{UnixSeconds, WebUrl};
use porter_fake_servers::dav::{Behaviour, Propagation, SyncCollection};
use porter_fake_servers::{DavHandle, FakeDav, Running};
use porter_http::{Header, Http, HttpError, HttpRequest, HttpResponse};
use std::future::Future;
use std::io;
use storage_webdav::{Clock, Dial, StreamHttp, StreamLimits, WebDavReplica};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const USER: &str = "ada";
pub const PASSWORD: &str = "app-password";
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

    fn dial(&self) -> impl Future<Output = Result<Tcp, HttpError>> + Send {
        let port = self.0;
        async move {
            TcpStream::connect(("127.0.0.1", port))
                .await
                .map(Tcp)
                .map_err(|_| HttpError::Unreachable)
        }
    }
}

/// Adds HTTP Basic credentials to every request, as the relay does on the far side.
#[derive(Debug)]
pub struct Basic<H>(pub H);

impl<H: Http> Http for Basic<H> {
    fn send(
        &self,
        mut request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> + Send {
        use base64::Engine;
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{PASSWORD}"));
        request
            .headers
            .push(Header::new("Authorization", format!("Basic {token}")));
        self.0.send(request)
    }
}

pub type Client = Basic<StreamHttp<TcpDial>>;
pub type Replica = WebDavReplica<Client>;

pub fn client(port: u16) -> Client {
    Basic(StreamHttp::new(TcpDial(port), StreamLimits::default()))
}

/// How the fake answers: with `sync-collection`, or as Nextcloud's files do, without.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Sync,
    Tree,
}

pub fn behaviour(flavor: Flavor) -> Behaviour {
    match flavor {
        Flavor::Sync => Behaviour::default(),
        Flavor::Tree => Behaviour {
            etag_propagation: Propagation::Up,
            sync_collection: SyncCollection::NotImplemented,
        },
    }
}

/// A running fake DAV server with the dataset's folder made.
pub struct Server {
    pub dav: Running<DavHandle>,
    pub port: u16,
    pub folder: WebUrl,
}

impl Server {
    pub async fn start(flavor: Flavor) -> Self {
        let dav = FakeDav::start(USER, PASSWORD).await.expect("fake dav");
        dav.set_behaviour(behaviour(flavor));
        dav.mkdir("ds");
        let port: u16 = dav
            .base_url()
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .expect("a port");
        let folder = WebUrl::parse(&format!("{}/dav/files/ds/", dav.base_url())).expect("url");
        Self { dav, port, folder }
    }

    /// A replica of the folder with feed pages of `page` changes; each call is a new replica
    /// (a new process, as far as anchors go).
    pub fn replica(&self, page: usize) -> Replica {
        WebDavReplica::new(client(self.port), &self.folder, Clock::fixed(NOW)).with_page(page)
    }
}
