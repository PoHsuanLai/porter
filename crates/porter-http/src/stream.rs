//! HTTP/1.1 over a byte stream a host dials, behind the `stream` feature. [`StreamHttp`] is an
//! [`Http`](crate::Http) over the connections a [`Dial`] opens: a connection is any
//! [`ByteStream`](porter_core::stream::ByteStream), so the same client serves an authenticated
//! relay's descriptor (the relay adds the credential and speaks TLS; the client never holds a
//! password) and a loopback socket in a test. The framing (`Content-Length`, chunks, or the end of
//! the stream) is in `wire`; only what a WebDAV or REST exchange needs: no redirects, no upgrade,
//! no compression.
//!
//! The module has no runtime and no socket of its own: the host implements `Dial`.

mod client;
mod wire;

pub use client::{Dial, StreamHttp, StreamLimits};
