//! Files on demand over one drive's listing, with no file system in it: what a FUSE adapter (or
//! any other way of showing the files) and a bus interface ask of a drive.
//!
//! - [`Listing`]: the drive's entries by id, parent and name, as the listing keeps them (syncd
//!   implements it on its drive journal).
//! - [`naming`]: the names items are shown by. A `/` in a name is shown as `／` (U+FF0F); of two
//!   items with one name in one folder the oldest by id keeps it and the others are "Name 2.ext",
//!   "Name 3.ext". One place, and the same every time the same listing is read.
//! - [`online`]: an online document (a Google Doc, Sheet, Slides, a shortcut) is a small link file
//!   `<name>.gdoc` (`.gsheet`, `.gslides`, `.glink`) whose content is made here,
//!   `{"url": "...", "doc_id": "..."}`; read-only and never cached.
//! - [`Drive`]: the content cache under one directory, one file per item keyed by its id and
//!   version and an index of what is there (the whole file or the ranges fetched), whether it is
//!   kept, its size and when it was last opened. A read waits for the chunk it needs while the
//!   rest is fetched in chunks through a [`Source`] ([`ReplicaSource`] over porter-sync's
//!   `Replica::fetch`), never the whole file in memory. Keep Downloaded, Remove Download,
//!   Download Now, eviction oldest-opened first past a [`CacheLimit`] (kept and open files are
//!   never evicted), and each item's [`Status`] as Files shows it.
//!
//! ```
//! use porter_drive::naming;
//!
//! assert_eq!(naming::shown_base("A/B report.pdf"), "A／B report.pdf");
//! assert_eq!(naming::numbered("Plan.docx", 2), "Plan 2.docx");
//! ```

mod drive;
mod index;
mod limit;
mod listing;
pub mod naming;
pub mod online;
mod ranges;
mod source;
#[cfg(feature = "testing")]
pub mod testing;
pub mod words;

pub use drive::{
    Attrs, Changed, Drive, DriveConfig, DriveError, ItemState, Node, NodeId, NodeKind, ROOT_INO,
    ReadError, Status,
};
pub use limit::{AUTOMATIC_FLOOR, CacheLimit};
pub use listing::{Entry, Listing, ListingError};
pub use porter_sync::RemoteId;
pub use source::{Fetching, ReplicaSource, Source, SourceError};
