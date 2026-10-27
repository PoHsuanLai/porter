//! porter's HTTP client over an accountd relay.
//!
//! A relay is an authenticated byte stream to one endpoint of a granted account
//! (`Tokens.OpenAuthenticated`): the daemon at its far end adds the credential and speaks TLS, so
//! the client writes plain HTTP/1.1 and never holds a password. [`RelayHttp`] runs hyper's HTTP/1
//! connection over the streams a [`Dial`] opens. It is an [`Http`](porter_http::Http) for whole
//! bodies, and [`RelayHttp::send_streaming`] sends a request body of any length as it is produced
//! and hands the response back as a [`RelayBody`] read piece by piece, so a transfer of a file is
//! never held in memory.
//!
//! The pieces, in one place:
//! - [`RelayHttp`], [`RelayLimits`], [`Dial`] and [`RelayBody`]: the client;
//! - [`RelayStream`]: the app's end of an accepted relay as a tokio stream;
//! - [`RelayDial`]: the [`Dial`] that asks accountd for a relay (`OpenAuthenticated`, or
//!   `OpenLinked` for a pre-authenticated link on another origin);
//! - [`Routed`]: the one [`Http`](porter_http::Http) that keeps the credentialed host and the
//!   linked hosts apart.

mod accounts;
mod client;
mod routed;
mod stream;

pub use accounts::{RelayDial, bare_url, dial_error};
pub use client::{Dial, RELAY_IDLE, RelayBody, RelayHttp, RelayLimits};
pub use routed::Routed;
pub use stream::RelayStream;
