//! The rig: a fake Google on loopback, reached through `StreamHttp` over a TCP dial with the
//! bearer added the way accountd's relay would add it. No network beyond loopback.
#![allow(dead_code)]

use porter_core::stream::ByteStream;
use porter_core::{UnixSeconds, WebUrl};
use porter_fake_servers::{FakeGoogle, GoogleHandle, Running};
use porter_http::{Header, Http, HttpError, HttpRequest, HttpResponse};
use std::future::Future;
use std::io;
use storage_gdrive::{Clock, Dial, GdriveReplica, StreamHttp, StreamLimits, Uploads};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const TOKEN: &str = "ya29.fake-access-token";
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
pub type Replica = GdriveReplica<Client>;

pub fn client(port: u16) -> Client {
    Bearer(StreamHttp::new(TcpDial(port), StreamLimits::default()))
}

/// A running fake Google.
pub struct Server {
    pub google: Running<GoogleHandle>,
    pub port: u16,
    pub base: WebUrl,
}

impl Server {
    pub async fn start() -> Self {
        let google = FakeGoogle::start_fixed(TOKEN).await.expect("fake google");
        let port: u16 = google
            .base_url()
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .expect("a port");
        let base = WebUrl::parse(&format!("{}/drive/v3", google.base_url())).expect("url");
        Self { google, port, base }
    }

    /// A replica of `folder` below the app data folder with pages of `page` items and these
    /// upload limits; each call is a new replica (a new process, as far as caches go).
    pub fn replica(&self, folder: &str, page: usize, uploads: Uploads) -> Replica {
        GdriveReplica::new(client(self.port), &self.base, folder, Clock::fixed(NOW))
            .with_page(page)
            .with_uploads(uploads)
    }
}
