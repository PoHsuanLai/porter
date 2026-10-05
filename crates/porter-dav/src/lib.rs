//! WebDAV for the families (decision D13), from mailo's `mail-pim::dav`: the requests PROPFIND
//! and REPORT take, and the parsed multistatus, sync-collection and quota answers. Pure: the
//! requests are [`porter_http::HttpRequest`] values and the replies are read from text, so
//! recorded fixtures drive every test. vCard and iCalendar values are the consumer's.

mod multistatus;
mod request;
mod sync;

pub use multistatus::{DavFault, Multistatus, Prop, PropStatus, Response, parse_multistatus};
pub use request::{Depth, propfind, report_sync_collection};
pub use sync::{SyncChange, SyncReply, parse_sync_collection};
