//! The seam.

use crate::error::HttpError;
use http::{Request, Response};
use std::future::Future;

/// Sends one request and returns the response, both the `http` crate's with a whole body.
/// Redirects, retries and timeouts are the implementation's, and are the same for every caller.
pub trait Http: Send + Sync {
    /// Sends `request`.
    fn send(
        &self,
        request: Request<Vec<u8>>,
    ) -> impl Future<Output = Result<Response<Vec<u8>>, HttpError>> + Send;
}
