//! porter's HTTP seam (design/31 D12): request and response values, and the [`Http`] trait every
//! family, discovery step and OAuth exchange sends through. A family never names a client; the
//! daemon hands it [`HyperHttp`] (feature `hyper`), a test hands it a fake server's. Bodies are
//! whole byte vectors: what porter sends and reads over HTTP (sign-in exchanges, PROPFIND
//! replies, model lists) is small, and a transfer of file content belongs to the replica that
//! streams it.

mod error;
mod headers;
mod http;
#[cfg(feature = "hyper")]
mod hyper_client;
mod message;
mod shared;
mod sleep;

pub use error::HttpError;
pub use headers::{Header, HeaderName, HeaderValue};
pub use http::Http;
#[cfg(feature = "hyper")]
pub use hyper_client::{HyperHttp, Limits, TokioSleep};
pub use message::{HttpRequest, HttpResponse, Method, Status};
pub use shared::SharedHttp;
pub use sleep::{NoSleep, SharedSleep, Sleep};
