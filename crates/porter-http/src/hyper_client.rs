//! The client over hyper (feature `hyper`).

use crate::error::HttpError;
use crate::http::Http;
use crate::message::{HttpRequest, HttpResponse};

/// hyper's client behind the [`Http`] seam: connection reuse, a timeout, a cap on the response
/// size, and TLS through rustls with the system's trust roots.
#[derive(Debug, Default)]
pub struct HyperHttp;

impl Http for HyperHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let _ = request;
        todo!(
            "hyper-util's legacy client over a tokio connector, rustls through hyper-rustls for \
             https, a timeout, a response size cap, redirects not followed"
        )
    }
}
