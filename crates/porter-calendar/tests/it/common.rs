//! The rigs: a fake DAV server on loopback reached through `StreamHttp` over a TCP dial with the
//! credential added the way accountd's relay would add it, and a scripted seam for the stores
//! that take JSON. No network beyond loopback.

use porter_core::WebUrl;
use porter_core::stream::ByteStream;
use porter_fake_servers::{DavHandle, FakeDav, Running};
use porter_http::stream::{Dial, StreamHttp, StreamLimits};
use porter_http::{Header, Http, HttpError, HttpRequest, HttpResponse, Status};
use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const USER: &str = "ada";
pub const PASSWORD: &str = "app-password";

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

/// A running fake DAV server and the URLs a test reaches it at.
pub struct Server {
    /// Held so the server runs for as long as the test does.
    _dav: Running<DavHandle>,
    pub port: u16,
    /// `/dav/`, the home the calendar is under.
    pub home: WebUrl,
    /// `/dav/calendar/`.
    pub calendar: String,
}

impl Server {
    pub async fn start() -> Self {
        let dav = FakeDav::start(USER, PASSWORD).await.expect("fake dav");
        let port: u16 = dav
            .base_url()
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .expect("a port");
        let home = WebUrl::parse(&format!("{}/dav/", dav.base_url())).expect("url");
        let calendar = format!("{}/dav/calendar/", dav.base_url());
        Self {
            _dav: dav,
            port,
            home,
            calendar,
        }
    }

    pub fn client(&self) -> Client {
        Basic(StreamHttp::new(TcpDial(self.port), StreamLimits::default()))
    }
}

/// A seam that answers from a script and keeps what it was sent.
#[derive(Debug, Default)]
pub struct Script {
    answers: Mutex<VecDeque<(u16, String)>>,
    pub sent: Mutex<Vec<HttpRequest>>,
}

impl Script {
    /// Answers each request in turn with `(status, body)`.
    pub fn new(answers: &[(u16, &str)]) -> Self {
        Self {
            answers: Mutex::new(
                answers
                    .iter()
                    .map(|(status, body)| (*status, (*body).to_owned()))
                    .collect(),
            ),
            sent: Mutex::new(Vec::new()),
        }
    }

    /// Each request sent, as `METHOD url`.
    pub fn lines(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|r| format!("{} {}", r.method.token(), r.url.as_str()))
            .collect()
    }

    /// The JSON body of the `at`th request.
    pub fn body(&self, at: usize) -> serde_json::Value {
        serde_json::from_slice(&self.sent.lock().unwrap()[at].body).expect("a JSON body")
    }

    /// The `name` header of the `at`th request.
    pub fn header(&self, at: usize, name: &str) -> Option<String> {
        self.sent.lock().unwrap()[at]
            .headers
            .iter()
            .find(|h| h.name.as_str().eq_ignore_ascii_case(name))
            .map(|h| h.value.0.clone())
    }
}

impl Http for &Script {
    fn send(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> + Send {
        self.sent.lock().unwrap().push(request);
        let answer = self.answers.lock().unwrap().pop_front();
        async move {
            let (status, body) = answer.expect("the script has an answer for every request");
            Ok(HttpResponse {
                status: Status(status),
                headers: Vec::new(),
                body: body.into_bytes(),
            })
        }
    }
}
