//! [`SharedHttp`]: any [`Http`], shared and cloneable. A family holds its client as one of these
//! so the closed enum over families needs no type parameter, and the daemon, an app hosting
//! porter and a test each hand in theirs. The cost is one boxed future per request, which is
//! nothing next to the request.

use crate::error::HttpError;
use crate::http::Http;
use crate::message::{HttpRequest, HttpResponse};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

type Reply<'a> = Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send + 'a>>;

trait DynHttp: Send + Sync {
    fn send_boxed(&self, request: HttpRequest) -> Reply<'_>;
}

impl<H: Http> DynHttp for H {
    fn send_boxed(&self, request: HttpRequest) -> Reply<'_> {
        Box::pin(self.send(request))
    }
}

/// Any [`Http`], shared.
#[derive(Clone)]
pub struct SharedHttp(Arc<dyn DynHttp>);

impl SharedHttp {
    /// Shares `http`.
    pub fn new(http: impl Http + 'static) -> Self {
        Self(Arc::new(http))
    }
}

impl std::fmt::Debug for SharedHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedHttp")
    }
}

impl Http for SharedHttp {
    fn send(
        &self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, HttpError>> + Send {
        self.0.send_boxed(request)
    }
}
