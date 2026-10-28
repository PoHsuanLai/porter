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
//! - **Changes made here** ([`Drive::open_writable`]): new files and folders, bytes written,
//!   renames and moves (saving by rename over a file among them) and removals are kept in the
//!   cache first, in a queue in its index, and sent through a [`Sink`] (porter-sync's
//!   `DriveWrite`) once a file is closed and quiet for [`QUIET`]. Lock files and the desktop's
//!   trash stay here ([`local_only`]).
//!
//! ```
//! use porter_drive::naming;
//!
//! assert_eq!(naming::shown_base("A/B report.pdf"), "A／B report.pdf");
//! assert_eq!(naming::numbered("Plan.docx", 2), "Plan 2.docx");
//! ```

mod drive;
mod hash;
mod index;
mod limit;
mod listing;
pub mod local_only;
pub mod naming;
pub mod online;
mod ranges;
mod reason;
mod sink;
mod source;
#[cfg(feature = "testing")]
pub mod testing;

pub use drive::{
    Attrs, Changed, Drive, DriveConfig, DriveError, FreeSpace, ItemState, Node, NodeId, NodeKind,
    QUIET, ROOT_INO, ReadError, Status, WriteError, WriteHandle,
};
pub use limit::{AUTOMATIC_FLOOR, CacheLimit};
pub use listing::{Entry, Listing, ListingError};
pub use porter_sync::{ContentHash, RemoteId};
pub use reason::Reason;
pub use sink::{Sending, Sink};
pub use source::{CopyError, Exporting, Fetching, ReplicaSource, Source, SourceError};
