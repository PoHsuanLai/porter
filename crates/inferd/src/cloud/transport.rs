//! A `model_http::Transport` over TLS: stoker's hyper client answers a `Tls` target with
//! `HttpError::Tls` (the first cloud backend is this one), so inferd carries its own: connect,
//! rustls handshake against the trusted roots, then one HTTP/1.1 exchange on the connection, as
//! `model-http`'s `hyper_client` does for a plain socket (this is that file's `send`, `talk` and
//! `respond`, with the handshake added and the body shaped first).
//!
//! The roots are the platform's (`rustls-native-certs`), with `webpki-roots` when the platform has
//! none. [`Roots::Only`] is the test seam: a scratch CA and nothing else. Certificate and name
//! checks are always on.
//!
//! The key is in the endpoint's `AuthHeader::Bearer`, whose `Secret` prints nothing, and the
//! header value is built `sensitive`. One transport is built for one turn and dropped with it.

use super::wire::BodyShape;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1;
use hyper::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, HOST, HeaderName as WireName, HeaderValue, RETRY_AFTER,
};
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use model_http::{
    AuthHeader, BodyKind, BodySink, ChunkFlow, Exchange, Framing, HttpEndpoint, HttpError,
    HttpStatus, HttpTarget, Proxy, RequestId, ResponseHead, RouteRoot, Timeouts, Transport, Verb,
    WaitMs, WaitSeconds,
};
use rustls_pki_types::{CertificateDer, ServerName};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::crypto::ring::default_provider;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

/// Which certificates a hosted model's server may be trusted by.
#[derive(Debug, Clone)]
pub enum Roots {
    /// The platform's roots, and `webpki-roots` when it offers none.
    Platform,
    /// Only these (a test's scratch CA).
    Only(Vec<CertificateDer<'static>>),
}

impl Roots {
    /// A connector trusting these roots. `Platform` reads the platform store from disk.
    pub fn connector(&self) -> TlsConnector {
        let mut store = RootCertStore::empty();
        match self {
            Roots::Platform => {
                let native = rustls_native_certs::load_native_certs();
                let (added, _) = store.add_parsable_certificates(native.certs);
                if added == 0 {
                    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                }
            }
            Roots::Only(roots) => {
                store.add_parsable_certificates(roots.iter().cloned());
            }
        }
        let config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring supports the default protocol versions")
            .with_root_certificates(store)
            .with_no_client_auth();
        TlsConnector::from(Arc::new(config))
    }
}

/// One turn's connection to a hosted model.
#[derive(Clone)]
pub struct TlsTransport {
    endpoint: HttpEndpoint,
    connector: TlsConnector,
    shape: BodyShape,
}

impl std::fmt::Debug for TlsTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The endpoint holds the key (redacted by its own `Debug`); show only where it goes.
        f.debug_struct("TlsTransport")
            .field("target", &self.endpoint.target)
            .finish_non_exhaustive()
    }
}

impl TlsTransport {
    /// Talks to `endpoint` (a `Tls` target, direct) through `connector`, shaping each body.
    pub fn new(endpoint: HttpEndpoint, connector: TlsConnector, shape: BodyShape) -> Self {
        Self {
            endpoint,
            connector,
            shape,
        }
    }
}

impl Transport for TlsTransport {
    fn exchange<K: BodySink>(
        &self,
        ex: &Exchange,
        sink: &mut K,
    ) -> impl Future<Output = Result<HttpStatus, HttpError>> + Send {
        let body = ex
            .body
            .as_ref()
            .map_or_else(Bytes::new, |b| Bytes::from(self.shape.apply(&b.0)));
        let parts = Parts {
            method: match ex.verb {
                Verb::Get => Method::GET,
                Verb::PostJson => Method::POST,
            },
            root: ex.root,
            path: ex.path.0.clone(),
            framing: ex.framing,
            content_type: ex.body.as_ref().map(|_| "application/json"),
            body,
        };
        send(&self.endpoint, &self.connector, parts, sink)
    }
}

/// What a request is.
struct Parts {
    method: Method,
    root: RouteRoot,
    path: String,
    framing: Framing,
    content_type: Option<&'static str>,
    body: Bytes,
}

async fn send<K: BodySink>(
    endpoint: &HttpEndpoint,
    connector: &TlsConnector,
    parts: Parts,
    sink: &mut K,
) -> Result<HttpStatus, HttpError> {
    let request = build(endpoint, parts)?;
    let timeouts = &endpoint.timeouts;
    let (Proxy::Direct, HttpTarget::Tls { host, port }) = (&endpoint.proxy, &endpoint.target)
    else {
        return Err(HttpError::Connect);
    };
    let tcp = within(
        timeouts.connect,
        TcpStream::connect((host.0.as_str(), port.0)),
    )
    .await?
    .map_err(|_| HttpError::Connect)?;
    let name = ServerName::try_from(host.0.clone()).map_err(|_| HttpError::Tls)?;
    let tls = within(timeouts.connect, connector.connect(name, tcp))
        .await?
        .map_err(|_| HttpError::Tls)?;
    talk(tls, request, timeouts, sink).await
}

async fn within<T>(wait: WaitMs, future: impl Future<Output = T>) -> Result<T, HttpError> {
    timeout(Duration::from_millis(u64::from(wait.0)), future)
        .await
        .map_err(|_| HttpError::Timeout)
}

/// The request line, headers and body against `endpoint`. A header value that cannot be written
/// (a newline in a secret) is a request that is never sent.
fn build(endpoint: &HttpEndpoint, parts: Parts) -> Result<Request<Full<Bytes>>, HttpError> {
    let path = match parts.root {
        RouteRoot::Base => format!("{}{}", endpoint.base.0, parts.path),
        RouteRoot::Server => parts.path,
    };
    let accept = match parts.framing {
        Framing::Sse => "text/event-stream",
        Framing::Ndjson => "application/x-ndjson",
        Framing::Whole => "application/json",
    };
    let HttpTarget::Tls { host, port } = &endpoint.target else {
        return Err(HttpError::Connect);
    };
    let mut builder = Request::builder()
        .method(parts.method)
        .uri(path)
        .header(HOST, format!("{}:{}", host.0, port.0))
        .header(ACCEPT, accept);
    if let Some(content_type) = parts.content_type {
        builder = builder.header(CONTENT_TYPE, content_type);
    }
    let secret = |name: WireName, value: &str| {
        HeaderValue::from_str(value)
            .map(|mut value| {
                value.set_sensitive(true);
                (name, value)
            })
            .map_err(|_| HttpError::Connect)
    };
    let mut headers = Vec::new();
    match &endpoint.auth {
        AuthHeader::None => {}
        AuthHeader::Bearer(token) => {
            headers.push(secret(AUTHORIZATION, &format!("Bearer {}", token.0))?);
        }
        AuthHeader::Header { name, value } => {
            let name = WireName::from_bytes(name.0.as_bytes()).map_err(|_| HttpError::Connect)?;
            headers.push(secret(name, &value.0)?);
        }
    }
    for extra in &endpoint.headers {
        let name = WireName::from_bytes(extra.name.0.as_bytes()).map_err(|_| HttpError::Connect)?;
        headers.push(secret(name, &extra.value.0)?);
    }
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder
        .body(Full::new(parts.body))
        .map_err(|_| HttpError::Connect)
}

/// One request on one connection; neither the handshake nor the exchange is spawned, so dropping
/// this future drops the socket.
async fn talk<IO, K>(
    io: IO,
    request: Request<Full<Bytes>>,
    timeouts: &Timeouts,
    sink: &mut K,
) -> Result<HttpStatus, HttpError>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    K: BodySink,
{
    let (mut sender, connection) = within(timeouts.connect, http1::handshake(TokioIo::new(io)))
        .await?
        .map_err(|_| HttpError::Connect)?;
    let work = respond(&mut sender, request, timeouts, sink);
    tokio::pin!(work);
    tokio::pin!(connection);
    let mut open = true;
    loop {
        tokio::select! {
            result = &mut work => return result,
            _ = &mut connection, if open => open = false,
        }
    }
}

async fn respond<K: BodySink>(
    sender: &mut http1::SendRequest<Full<Bytes>>,
    request: Request<Full<Bytes>>,
    timeouts: &Timeouts,
    sink: &mut K,
) -> Result<HttpStatus, HttpError> {
    let response = within(timeouts.first_byte, sender.send_request(request))
        .await?
        .map_err(|_| HttpError::Broken)?;
    let head = head_of(&response);
    let status = head.status;
    let mut body = response.into_body();
    let mut flow = sink.head(&head);
    while flow == ChunkFlow::Continue {
        match within(timeouts.idle, body.frame()).await? {
            None => break,
            Some(Err(_)) => return Err(HttpError::Broken),
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    flow = sink.chunk(&data);
                }
            }
        }
    }
    if (200..300).contains(&status.0) {
        Ok(status)
    } else {
        Err(HttpError::Rejected)
    }
}

fn head_of<B>(response: &Response<B>) -> ResponseHead {
    let text = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    ResponseHead {
        status: HttpStatus(response.status().as_u16()),
        body: BodyKind::of(&text(CONTENT_TYPE.as_str()).unwrap_or_default()),
        retry_after: text(RETRY_AFTER.as_str()).and_then(|v| WaitSeconds::from_header(&v)),
        request_id: text("x-request-id").and_then(|v| RequestId::new(v).ok()),
    }
}
