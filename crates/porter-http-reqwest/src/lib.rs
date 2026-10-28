//! porter's one direct-connection HTTP client: [`ReqwestHttp`], reqwest behind the
//! [`Http`](porter_http::Http) seam.
//!
//! TLS is rustls on `ring` with the platform's trust roots (plus any the caller adds, which is
//! how a test trusts its scratch CA); certificate checks are never off. Plain `http` is for this
//! computer only: a request to any other host over `http` is refused before it is dialled.
//! Redirects are not followed (a caller that wants one reads `Location`), the whole exchange has
//! a timeout, and a response longer than the cap is `TooLarge`.
//!
//! This is the only crate in porter that names reqwest (`scripts/check-boundary.sh`).

use porter_http::HttpError;
use porter_http::http::{HeaderMap, Request, Response, StatusCode, Uri};
use porter_ids::{Origin, UrlScheme};
use reqwest::redirect::Policy;
use reqwest::{Certificate, Client};
use std::error::Error;
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

/// reqwest behind the [`Http`](porter_http::Http) seam: connection reuse, a timeout, a cap on
/// the response size, and TLS through rustls with the system's trust roots.
#[derive(Debug, Clone)]
pub struct ReqwestHttp {
    client: Client,
    limits: Limits,
}

impl Default for ReqwestHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestHttp {
    /// A client trusting the platform's roots.
    pub fn new() -> Self {
        Self::with_extra_roots(Vec::new())
    }

    /// A client trusting the platform's roots and these DER certificates (a private CA).
    /// A certificate that is not one is skipped.
    pub fn with_extra_roots(extra: Vec<Vec<u8>>) -> Self {
        let builder = extra
            .iter()
            .filter_map(|der| Certificate::from_der(der).ok())
            .fold(
                Client::builder()
                    .redirect(Policy::none())
                    .pool_idle_timeout(Duration::from_secs(30)),
                |builder, root| builder.add_root_certificate(root),
            );
        Self {
            client: builder
                .build()
                .unwrap_or_else(|_| unreachable!("a client with rustls and no proxy builds")),
            limits: Limits::default(),
        }
    }

    /// The same client with other limits.
    #[must_use]
    pub fn with_limits(self, limits: Limits) -> Self {
        Self { limits, ..self }
    }

    /// The request as reqwest sends it, with the exchange's timeout.
    fn outgoing(
        &self,
        request: Request<Vec<u8>>,
        timeout: Option<Duration>,
    ) -> Result<reqwest::Request, HttpError> {
        in_the_clear_only_here(request.uri())?;
        let mut outgoing = reqwest::Request::try_from(request).map_err(fault)?;
        *outgoing.timeout_mut() = timeout;
        Ok(outgoing)
    }

    /// Sends `request` and gives the response to `sink` as it arrives, without holding it. The
    /// body has no size cap and no whole-exchange timeout; `idle` is how long the connection, the
    /// head or any one piece may take. Redirects are not followed, so the caller reads
    /// `Location` from the head and asks again.
    ///
    /// # Errors
    /// [`HttpError`] when no response head arrived or the body broke off.
    pub async fn stream<S: BodySink>(
        &self,
        request: Request<Vec<u8>>,
        idle: Duration,
        sink: &mut S,
    ) -> Result<StatusCode, HttpError> {
        let outgoing = self.outgoing(request, None)?;
        let mut response = tokio::time::timeout(idle, self.client.execute(outgoing))
            .await
            .map_err(|_| HttpError::TimedOut)?
            .map_err(fault)?;
        let status = response.status();
        if sink.head(status, response.headers()) == Flow::Stop {
            return Ok(status);
        }
        loop {
            let next = tokio::time::timeout(idle, response.chunk())
                .await
                .map_err(|_| HttpError::TimedOut)?
                .map_err(fault)?;
            let Some(data) = next else {
                return Ok(status);
            };
            if sink.chunk(&data) == Flow::Stop {
                return Ok(status);
            }
        }
    }

    async fn exchange(&self, request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, HttpError> {
        let outgoing = self.outgoing(request, Some(self.limits.timeout))?;
        let mut response = self.client.execute(outgoing).await.map_err(fault)?;
        let mut bytes = Vec::new();
        while let Some(data) = response.chunk().await.map_err(fault)? {
            if bytes.len() + data.len() > self.limits.max_body {
                return Err(HttpError::TooLarge);
            }
            bytes.extend_from_slice(&data);
        }
        let mut answer = Response::new(bytes);
        *answer.status_mut() = response.status();
        *answer.version_mut() = response.version();
        *answer.headers_mut() = response.headers().clone();
        Ok(answer)
    }
}

impl porter_http::Http for ReqwestHttp {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, HttpError> {
        tokio::time::timeout(self.limits.timeout, self.exchange(request))
            .await
            .unwrap_or(Err(HttpError::TimedOut))
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

/// Plain `http` goes to this computer only: a request to any other host over `http` is `Tls`
/// and is never dialled, and a scheme that is not HTTP is `Malformed`.
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

/// What a reqwest error comes to: a timeout, a certificate or handshake refusal, no route, or
/// garbage.
fn fault(error: reqwest::Error) -> HttpError {
    match (error.is_timeout(), is_tls(&error), error.is_connect()) {
        (true, _, _) => HttpError::TimedOut,
        (false, true, _) => HttpError::Tls,
        (false, false, true) => HttpError::Unreachable,
        (false, false, false) => HttpError::Malformed,
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

impl porter_http::Sleep for TokioSleep {
    async fn sleep(&self, how_long: Duration) {
        tokio::time::sleep(how_long).await;
    }
}
