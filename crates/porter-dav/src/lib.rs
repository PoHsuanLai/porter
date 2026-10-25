//! WebDAV for the families (decision D13), over [`dav_proto`]: the requests PROPFIND and REPORT
//! take as [`porter_http::HttpRequest`] values, and the discovery and quota answers read from a
//! parsed multistatus. The bodies, the multistatus and sync-collection parsers and the property
//! names are dav-proto's; vCard and iCalendar values are the consumer's.

mod discover;
mod request;

pub use discover::{Home, Quota, collections, current_user_principal, home_set, quota};
pub use request::{propfind, report_sync_collection};

// moved to dav_proto; re-export kept for one batch
#[doc(hidden)]
pub use dav_proto::{
    Depth, Multistatus, Prop, PropStatus, Response, SyncChange, SyncEnd, SyncLevel, SyncReply,
    names, parse_multistatus, parse_sync_collection, token_expired,
};

// moved to dav_proto::Error; re-export kept for one batch
#[doc(hidden)]
pub use dav_proto::Error as DavFault;
