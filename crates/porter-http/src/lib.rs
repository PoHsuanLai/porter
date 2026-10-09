//! porter's HTTP seam (design/31 D12): request and response values, and the [`Http`] trait every
//! family, discovery step and OAuth exchange sends through. A family never names a client; the
//! daemon hands it [`HyperHttp`] (feature `hyper`), a test hands it a fake server's. Bodies are
//! whole byte vectors: what porter sends and reads over HTTP (sign-in exchanges, PROPFIND
//! replies, model lists) is small, and a transfer of file content belongs to the replica that
//! streams it.
//!
//! Feature `stream` adds [`stream`]: HTTP/1.1 framed over a byte stream a host dials (an
//! authenticated relay's descriptor, say), for a replica that streams file content.
//!
//! Requests and responses are plain values (no feature needed):
//!
//! ```
//! use porter_core::WebUrl;
//! use porter_http::{Header, HttpRequest, HttpResponse, Method, Status};
//!
//! let url = WebUrl::parse("https://cloud.example.org/remote.php/dav/").expect("a web url");
//! let request = HttpRequest::new(Method::Propfind, url)
//!     .with_header("Depth", "1")
//!     .with_body("<propfind/>");
//! assert_eq!(request.method.token(), "PROPFIND");
//! assert_eq!(request.headers.len(), 1);
//!
//! let response = HttpResponse {
//!     status: Status(429),
//!     headers: vec![Header::new("Retry-After", "30")],
//!     body: Vec::new(),
//! };
//! assert!(!response.status.is_success());
//! assert_eq!(response.retry_after_seconds(), Some(30));
//! ```

mod error;
mod headers;
mod http;
#[cfg(feature = "hyper")]
mod hyper_client;
mod message;
mod shared;
mod sleep;
#[cfg(feature = "stream")]
pub mod stream;

pub use error::HttpError;
pub use headers::{Header, HeaderName, HeaderValue};
pub use http::Http;
#[cfg(feature = "hyper")]
pub use hyper_client::{HyperHttp, Limits, TokioSleep};
pub use message::{HttpRequest, HttpResponse, Method, Status, web_url};
pub use shared::SharedHttp;
pub use sleep::{NoSleep, SharedSleep, Sleep};
