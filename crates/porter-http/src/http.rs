//! The seam.

use crate::error::HttpError;
use crate::message::{HttpRequest, HttpResponse};
use std::future::Future;

/// Sends one request and returns the response. Redirects, retries and timeouts are the
/// implementation's, and are the same for every caller.
pub trait Http: Send + Sync {
    /// Sends `request`.
    fn send(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> + Send;
}
