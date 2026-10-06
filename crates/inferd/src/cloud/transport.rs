//! A hosted model's transport: stoker's `HttpClient` (which speaks TLS for a `Tls` target, behind
//! `model-http`'s `tls` feature, with `TlsRoots` choosing the trusted roots) wrapped so each body
//! is first shaped by porter's request policy ([`BodyShape`]: sampling, a model's reasoning
//! field). The shaping is inferd's and stays here; the connection, the handshake and the
//! certificate and name checks are stoker's and always on.
//!
//! The key is in the endpoint's `AuthHeader::Bearer`, whose `Secret` prints nothing. One transport
//! is built for one turn and dropped with it.

use super::wire::BodyShape;
use model_http::{
    BodySink, Exchange, HttpClient, HttpEndpoint, HttpError, HttpStatus, JsonBody, TlsRoots,
    Transport,
};
use std::future::Future;

/// One turn's connection to a hosted model.
#[derive(Clone)]
pub struct ShapedTransport {
    client: HttpClient,
    shape: BodyShape,
}

impl std::fmt::Debug for ShapedTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The endpoint holds the key (redacted by its own `Debug`); show only where it goes.
        f.debug_struct("ShapedTransport")
            .field("target", &self.client.endpoint().target)
            .finish_non_exhaustive()
    }
}

impl ShapedTransport {
    /// Talks to `endpoint` (a `Tls` target, direct) trusting `roots`, shaping each body.
    pub fn new(endpoint: HttpEndpoint, roots: TlsRoots, shape: BodyShape) -> Self {
        Self {
            client: HttpClient::with_roots(endpoint, roots),
            shape,
        }
    }
}

impl Transport for ShapedTransport {
    fn exchange<K: BodySink>(
        &self,
        ex: &Exchange,
        sink: &mut K,
    ) -> impl Future<Output = Result<HttpStatus, HttpError>> + Send {
        let shaped = Exchange {
            body: ex
                .body
                .as_ref()
                .map(|body| JsonBody(self.shape.apply(&body.0))),
            ..ex.clone()
        };
        let client = self.client.clone();
        async move { client.exchange(&shaped, sink).await }
    }
}
