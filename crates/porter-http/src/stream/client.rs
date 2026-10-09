//! [`StreamHttp`]: the HTTP seam over connections a [`Dial`] opens. In syncd a connection is the
//! descriptor `Tokens.OpenAuthenticated` returned: the relay at its far end adds the credential
//! and speaks TLS, so this client writes plain HTTP/1.1 and never holds a password. In a test a
//! dial is a loopback socket to the fake server.

use super::wire::{Exchange, encode, io_fault, read_response};
use crate::{Http, HttpError, HttpRequest, HttpResponse};
use porter_core::stream::ByteStream;
use std::future::Future;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Opens connections to the endpoint, each already authenticated.
pub trait Dial: Send + Sync {
    /// What a connection reads and writes.
    type Stream: ByteStream;

    /// A new connection.
    fn dial(&self) -> impl Future<Output = Result<Self::Stream, HttpError>> + Send;
}

/// How [`StreamHttp`] behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamLimits {
    /// The most a response body may hold.
    pub max_body: usize,
    /// How many idle connections are kept for the next request.
    pub idle: usize,
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            max_body: 256 * 1024 * 1024,
            idle: 2,
        }
    }
}

/// An [`Http`] over a [`Dial`]: connections are reused while the server keeps them open, and a
/// request that finds a kept connection dead is sent once more on a new one.
#[derive(Debug)]
pub struct StreamHttp<D: Dial> {
    dial: D,
    limits: StreamLimits,
    idle: Mutex<Vec<D::Stream>>,
}

impl<D: Dial> StreamHttp<D> {
    /// A client dialling with `dial`.
    pub fn new(dial: D, limits: StreamLimits) -> Self {
        Self {
            dial,
            limits,
            idle: Mutex::new(Vec::new()),
        }
    }

    fn idle(&self) -> MutexGuard<'_, Vec<D::Stream>> {
        // Every critical section is a push or a pop.
        self.idle.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn exchange(
        &self,
        stream: &mut D::Stream,
        request: &HttpRequest,
        bytes: &[u8],
    ) -> Result<Exchange, HttpError> {
        stream
            .write_all(bytes)
            .await
            .map_err(|error| io_fault(&error))?;
        read_response(stream, request.method, self.limits.max_body).await
    }

    fn keep(&self, stream: D::Stream, exchange: &Exchange) {
        let mut idle = self.idle();
        if exchange.reusable && idle.len() < self.limits.idle {
            idle.push(stream);
        }
    }
}

impl<D: Dial> Http for StreamHttp<D> {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let authority = authority_of(&request);
        let bytes = encode(&request, &authority);
        let kept = self.idle().pop();
        if let Some(mut stream) = kept {
            // A connection that is gone is one the server closed while it sat idle; any other
            // failure is the server's answer and is not sent twice.
            match self.exchange(&mut stream, &request, &bytes).await {
                Ok(exchange) => {
                    self.keep(stream, &exchange);
                    return Ok(exchange.response);
                }
                Err(HttpError::Unreachable) => {}
                Err(other) => return Err(other),
            }
        }
        let mut stream = self.dial.dial().await?;
        let exchange = self.exchange(&mut stream, &request, &bytes).await?;
        self.keep(stream, &exchange);
        Ok(exchange.response)
    }
}

/// The `host[:port]` of the request's URL as it was written.
fn authority_of(request: &HttpRequest) -> String {
    let text = request.url.as_str();
    let after = text.find("://").map_or(0, |at| at + 3);
    let end = text[after..]
        .find(['/', '?'])
        .map_or(text.len(), |at| after + at);
    text[after..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Method;
    use porter_core::WebUrl;

    #[test]
    fn the_host_is_the_authority_the_url_named() {
        const CASES: &[(&str, &str)] = &[
            ("http://127.0.0.1:8080/remote.php/dav/", "127.0.0.1:8080"),
            ("https://cloud.example.org/dav/a?x=1", "cloud.example.org"),
            ("https://cloud.example.org?x=1", "cloud.example.org"),
        ];
        for (url, want) in CASES {
            let request = HttpRequest::new(Method::Get, WebUrl::parse(url).expect("url"));
            assert_eq!(authority_of(&request), *want, "{url}");
        }
    }
}
