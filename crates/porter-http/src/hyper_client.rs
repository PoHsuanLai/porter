//! The client over hyper (feature `hyper`).
//!
//! TLS is rustls on `ring` with the platform's trust roots (plus any the caller adds, which is
//! how a test trusts its scratch CA); certificate checks are never off. Plain `http` is for this
//! computer only: a request to any other host over `http` is refused before it is dialled.
//! Redirects are not followed (a caller that wants one reads `Location`), the whole exchange has
//! a timeout, and a response longer than the cap is `TooLarge`.

use crate::error::HttpError;
use crate::headers::Header;
use crate::http::Http;
use crate::message::{HttpRequest, HttpResponse, Status};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use porter_core::UrlScheme;
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
        let origin = request.url.origin();
        match origin.scheme {
            UrlScheme::Https => {}
            UrlScheme::Http if origin.is_loopback() => {}
            // Plain HTTP to another computer would send a credential in the clear.
            UrlScheme::Http => return Err(HttpError::Tls),
            _ => return Err(HttpError::Malformed),
        }
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
        Ok(HttpResponse {
            status: Status(head.status.as_u16()),
            headers: head
                .headers
                .iter()
                .filter_map(|(name, value)| Some(Header::new(name.as_str(), value.to_str().ok()?)))
                .collect(),
            body: bytes,
        })
    }
}

impl Http for HyperHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        tokio::time::timeout(self.limits.timeout, self.exchange(request))
            .await
            .unwrap_or(Err(HttpError::TimedOut))
    }
}

fn build(request: HttpRequest) -> Result<hyper::Request<Full<Bytes>>, HttpError> {
    let method = hyper::Method::from_bytes(request.method.token().as_bytes())
        .map_err(|_| HttpError::Malformed)?;
    let builder = request.headers.iter().try_fold(
        hyper::Request::builder()
            .method(method)
            .uri(request.url.as_str()),
        |builder, header| {
            let name = hyper::header::HeaderName::from_bytes(header.name.as_str().as_bytes())
                .map_err(|_| HttpError::Malformed)?;
            let value = hyper::header::HeaderValue::from_str(&header.value.0)
                .map_err(|_| HttpError::Malformed)?;
            Ok::<_, HttpError>(builder.header(name, value))
        },
    )?;
    builder
        .body(Full::new(Bytes::from(request.body)))
        .map_err(|_| HttpError::Malformed)
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
