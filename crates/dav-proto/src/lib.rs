//! WebDAV (RFC 4918), CalDAV (RFC 4791) and CardDAV (RFC 6352) as text, with no IO. Request
//! bodies are built as `String`s and replies are read from `&str`, so the caller sends them with
//! whatever HTTP client it has, and recorded bodies drive every test.
//!
//! - [`parse_multistatus`] reads a `207 Multi-Status` body into [`Response`]s, each property
//!   with the status of its `propstat` ([`PropStatus`]), so a property the server could not
//!   return is marked rather than lost.
//! - [`parse_sync_collection`] reads an RFC 6578 `sync-collection` reply: a 404 response is a
//!   removal, and a 507 response means the server cut the report short, which is
//!   [`SyncEnd::Truncated`] with the token to continue from. [`token_expired`] reads the
//!   `valid-sync-token` precondition of a failed report.
//! - [`propfind`], [`sync_collection`] and [`multiget`] build the request bodies; the `Depth`
//!   header and the method are the caller's ([`Depth::as_str`]).
//! - [`carddav::Props`] is a typed view of one response's CardDAV properties.
//! - With the `http` feature, the `http` module builds the same requests as
//!   `http::Request<String>`.
//!
//! Property names are namespace-qualified strings (`DAV:getetag`,
//! `urn:ietf:params:xml:ns:carddav:addressbook-home-set`); [`names`] holds the common ones.
//!
//! ```
//! use dav_proto::{SyncChange, SyncEnd, parse_multistatus, parse_sync_collection};
//!
//! let body = r#"<d:multistatus xmlns:d="DAV:">
//!   <d:response><d:href>/book/a.vcf</d:href>
//!     <d:propstat><d:prop><d:getetag>"e1"</d:getetag></d:prop>
//!     <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
//!   <d:response><d:href>/book/</d:href>
//!     <d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response>
//!   <d:sync-token>tok-2</d:sync-token>
//! </d:multistatus>"#;
//! let reply = parse_sync_collection(&parse_multistatus(body)?);
//! assert_eq!(
//!     reply.changes,
//!     [SyncChange::Changed { href: "/book/a.vcf".into(), etag: "\"e1\"".into() }]
//! );
//! // Cut short: ask again from this token now for the rest.
//! assert_eq!(reply.end, SyncEnd::Truncated("tok-2".into()));
//! # Ok::<(), dav_proto::Error>(())
//! ```

#![warn(missing_docs)]

mod body;
pub mod carddav;
mod error;
#[cfg(feature = "http")]
pub mod http;
mod multistatus;
pub mod names;
mod sync;

pub use body::{Depth, Multiget, SyncLevel, escape, multiget, propfind, sync_collection};
pub use error::Error;
pub use multistatus::{
    Multistatus, Prop, PropStatus, Response, parse_multistatus, parse_status_line,
};
pub use sync::{SyncChange, SyncEnd, SyncReply, parse_sync_collection, token_expired};
