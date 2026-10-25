//! The client over hyper (feature `hyper`).
//!
//! TLS is rustls on `ring` with the platform's trust roots (plus any the caller adds, which is
//! how a test trusts its scratch CA); certificate checks are never off. Plain `http` is for this
//! computer only: a request to any other host over `http` is refused before it is dialled.
//! Redirects are not followed (a caller that wants one reads `Location`), the whole exchange has
//! a timeout, and a response longer than the cap is `TooLarge`.

use crate::error::HttpError;
use crate::message::{HttpRequest, HttpResponse};
use crate::seam::Http;
use http::{HeaderMap, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use porter_core::{Origin, UrlScheme};
use rustls::crypto::ring::default_provider;
use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::CertificateDer;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

/// How long a request may take and how much of a response is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The whole exchange: connecting, the handshake, the request and the whole response.
    pub timeout: Duration,
    /// The most a response body may hold, in bytes.
    pub max_body: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_body: 8 * 1024 * 1024,
        }
    }
}

/// hyper's client behind the [`Http`] seam: connection reuse, a timeout, a cap on the response
/// size, and TLS through rustls with the system's trust roots.
#[derive(Debug, Clone)]
pub struct HyperHttp {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    limits: Limits,
}

impl Default for HyperHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl HyperHttp {
    /// A client trusting the platform's roots.
    pub fn new() -> Self {
        Self::with_extra_roots(Vec::new())
    }

    /// A client trusting the platform's roots and these DER certificates (a private CA).
    /// A certificate that is not one is skipped.
    pub fn with_extra_roots(extra: Vec<Vec<u8>>) -> Self {
        let mut roots = RootCertStore::empty();
        let native = rustls_native_certs::load_native_certs();
        roots.add_parsable_certificates(native.certs);
        roots.add_parsable_certificates(extra.into_iter().map(CertificateDer::from));
        let config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap_or_else(|_| unreachable!("ring supports the default protocol versions"))
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = HttpsConnectorBuilder::new()
            .with_tls_config(config)
            .https_or_http()
            .enable_http1()
            .build();
        Self {
            client: Client::builder(TokioExecutor::new())
                .pool_idle_timeout(Duration::from_secs(30))
                .build(connector),
            limits: Limits::default(),
        }
    }

    /// The same client with other limits.
    pub fn with_limits(self, limits: Limits) -> Self {
        Self { limits, ..self }
    }

    async fn exchange(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let outgoing = build(request)?;
        let response = self.client.request(outgoing).await.map_err(client_fault)?;
        let (head, mut body) = response.into_parts();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| HttpError::Malformed)?;
            if let Some(data) = frame.data_ref() {
                if bytes.len() + data.len() > self.limits.max_body {
                    return Err(HttpError::TooLarge);
                }
                bytes.extend_from_slice(data);
            }
        }
        Ok(Response::from_parts(head, bytes))
    }
}

/// What the reader of a streamed body says after each piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Go on reading.
    Continue,
    /// Stop: the rest is not wanted. The connection is dropped.
    Stop,
}

/// Receives a response as it arrives, for a body too long to hold: the head once, then each
/// piece of the body in turn.
pub trait BodySink: Send {
    /// The status and headers, before any of the body. `Stop` reads no body.
    fn head(&mut self, status: StatusCode, headers: &HeaderMap) -> Flow;
    /// The next piece of the body.
    fn chunk(&mut self, bytes: &[u8]) -> Flow;
}

impl HyperHttp {
    /// Sends `request` and gives the response to `sink` as it arrives, without holding it. The
    /// body has no size cap and no whole-exchange timeout; `idle` is how long the connection, the
    /// head or any one piece may take. Redirects are not followed, so the caller reads
    /// `Location` from the head and asks again.
    ///
    /// # Errors
    /// [`HttpError`] when no response head arrived or the body broke off.
    pub async fn stream<S: BodySink>(
        &self,
        request: HttpRequest,
        idle: Duration,
        sink: &mut S,
    ) -> Result<StatusCode, HttpError> {
        let outgoing = build(request)?;
        let response = tokio::time::timeout(idle, self.client.request(outgoing))
            .await
            .map_err(|_| HttpError::TimedOut)?
            .map_err(client_fault)?;
        let (head, mut body) = response.into_parts();
        let status = head.status;
        if sink.head(status, &head.headers) == Flow::Stop {
            return Ok(status);
        }
        loop {
            let next = tokio::time::timeout(idle, body.frame())
                .await
                .map_err(|_| HttpError::TimedOut)?;
            let Some(frame) = next else {
                return Ok(status);
            };
            let frame = frame.map_err(|_| HttpError::Malformed)?;
            if let Some(data) = frame.data_ref()
                && sink.chunk(data) == Flow::Stop
            {
                return Ok(status);
            }
        }
    }
}

impl Http for HyperHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        tokio::time::timeout(self.limits.timeout, self.exchange(request))
            .await
            .unwrap_or(Err(HttpError::TimedOut))
    }
}

/// The request as hyper sends it. Plain `http` goes to this computer only: a request to any
/// other host over `http` is `Tls` and is never dialled, and a scheme that is not HTTP is
/// `Malformed`.
fn build(request: HttpRequest) -> Result<hyper::Request<Full<Bytes>>, HttpError> {
    in_the_clear_only_here(request.uri())?;
    Ok(request.map(|body| Full::new(Bytes::from(body))))
}

fn in_the_clear_only_here(uri: &Uri) -> Result<(), HttpError> {
    match uri.scheme_str() {
        Some("https") => Ok(()),
        Some("http") => {
            let host = uri.host().unwrap_or_default();
            let origin = Origin {
                scheme: UrlScheme::Http,
                host: host.trim_matches(['[', ']']).to_ascii_lowercase(),
                port: uri.port_u16().unwrap_or(UrlScheme::Http.default_port()),
            };
            match origin.is_loopback() {
                true => Ok(()),
                false => Err(HttpError::Tls),
            }
        }
        _ => Err(HttpError::Malformed),
    }
}

/// What a client error comes to: a certificate or handshake refusal, no route, or garbage.
fn client_fault(error: hyper_util::client::legacy::Error) -> HttpError {
    match (is_tls(&error), error.is_connect()) {
        (true, _) => HttpError::Tls,
        (false, true) => HttpError::Unreachable,
        (false, false) => HttpError::Malformed,
    }
}

/// Whether a rustls error is anywhere in the error's chain (tokio-rustls wraps it in `io::Error`s,
/// which do not pass what they wrap on as a `source`).
fn is_tls(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(err) = current {
        if err.is::<rustls::Error>() || wraps_tls(err) {
            return true;
        }
        current = err.source();
    }
    false
}

fn wraps_tls(error: &(dyn Error + 'static)) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .and_then(|io| io.get_ref())
        .is_some_and(|inner| inner.is::<rustls::Error>() || wraps_tls(inner))
}

/// Sleeps on tokio's timer: what a sign-in that polls waits with (the daemon hands it in).
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSleep;

impl crate::sleep::Sleep for TokioSleep {
    async fn sleep(&self, how_long: Duration) {
        tokio::time::sleep(how_long).await;
    }
}
