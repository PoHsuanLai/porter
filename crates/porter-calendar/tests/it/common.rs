//! The rigs: a fake DAV server on loopback reached through `StreamHttp` over a TCP dial with the
//! credential added the way accountd's relay would add it, and a scripted seam for the stores
//! that take JSON. No network beyond loopback.

use porter_fake_servers::{DavHandle, FakeDav, Running};
use porter_http::http::header::{AUTHORIZATION, HeaderValue};
use porter_http::http::{Request, Response};
use porter_http::{HeaderText, Http, HttpError};
use porter_http_relay::{Dial, RelayHttp, RelayLimits};
use porter_ids::WebUrl;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::Mutex;
use tokio::net::TcpStream;

pub const USER: &str = "ada";
pub const PASSWORD: &str = "app-password";

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
        Basic(RelayHttp::new(TcpDial(self.port), RelayLimits::default()))
    }
}

/// A seam that answers from a script and keeps what it was sent.
#[derive(Debug, Default)]
pub struct Script {
    answers: Mutex<VecDeque<(u16, String)>>,
    pub sent: Mutex<Vec<Request<Vec<u8>>>>,
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
            .map(|r| format!("{} {}", r.method(), r.uri()))
            .collect()
    }

    /// The JSON body of the `at`th request.
    pub fn body(&self, at: usize) -> serde_json::Value {
        serde_json::from_slice(self.sent.lock().unwrap()[at].body()).expect("a JSON body")
    }

    /// The `name` header of the `at`th request.
    pub fn header(&self, at: usize, name: &str) -> Option<String> {
        self.sent.lock().unwrap()[at]
            .header(name)
            .map(str::to_owned)
    }
}

impl Http for &Script {
    fn send(
        &self,
        request: Request<Vec<u8>>,
    ) -> impl Future<Output = Result<Response<Vec<u8>>, HttpError>> + Send {
        self.sent.lock().unwrap().push(request);
        let answer = self.answers.lock().unwrap().pop_front();
        async move {
            let (status, body) = answer.expect("the script has an answer for every request");
            Ok(Response::builder()
                .status(status)
                .body(body.into_bytes())
                .expect("a response"))
        }
    }
}
