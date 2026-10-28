//! The rig: a fake DAV server on loopback, reached through `RelayHttp` over a TCP dial with
//! the credential added the way accountd's relay would add it. No network beyond loopback.
#![allow(dead_code)]

use porter_fake_servers::dav::Behaviour;
use porter_fake_servers::{DavHandle, FakeDav, Running};
use porter_http::http::header::{AUTHORIZATION, HeaderValue};
use porter_http::http::{Request, Response};
use porter_http::{Http, HttpError};
use porter_http_relay::{Dial, RelayHttp, RelayLimits};
use porter_ids::{UnixSeconds, WebUrl};
use porter_sync::Clock;
use std::future::Future;
use storage_webdav::WebDavReplica;
use tokio::net::TcpStream;

pub const USER: &str = "ada";
pub const PASSWORD: &str = "app-password";
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

/// Adds HTTP Basic credentials to every request, as the relay does on the far side.
#[derive(Debug)]
pub struct Basic<H>(pub H);

impl<H: Http> Http for Basic<H> {
    fn send(
        &self,
        mut request: Request<Vec<u8>>,
    ) -> impl Future<Output = Result<Response<Vec<u8>>, HttpError>> + Send {
        use base64::Engine;
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{USER}:{PASSWORD}"));
        let value = HeaderValue::try_from(format!("Basic {token}")).expect("a header value");
        request.headers_mut().append(AUTHORIZATION, value);
        self.0.send(request)
    }
}

pub type Client = Basic<RelayHttp<TcpDial>>;
pub type Replica = WebDavReplica<Client>;

pub fn client(port: u16) -> Client {
    Basic(RelayHttp::new(TcpDial(port), RelayLimits::default()))
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
            propagate_etags: true,
            serves_sync_collection: false,
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
        WebDavReplica::new(client(self.port), &self.folder, Clock::fixed(NOW), "n1").with_page(page)
    }
}
