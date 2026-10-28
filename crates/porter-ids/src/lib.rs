//! Values that name nothing of accounts: the grammar of an id and the [`slug_id!`] macro that
//! declares one, units, web URLs and origins, the time as a seam, and percent-encoding. Each is
//! parsed once where it enters, so the rest of the code holds a value that is already safe in a
//! file name and, after [`object_segment`], in a D-Bus object path:
//!
//! ```
//! use porter_ids::{is_id, object_segment};
//!
//! assert!(is_id("work.nextcloud"));
//! // On the bus the id is one path segment: `.` and `-` become `_`.
//! assert_eq!(object_segment("work.nextcloud"), "work_nextcloud");
//! // Capital letters are not in the grammar.
//! assert!(!is_id("Work"));
//! ```
//!
//! No runtime, no file system, no I/O. The time is one seam, [`clock`]: its
//! [`clock::SystemClock`] reads the wall clock only when a daemon calls it.

pub mod clock;
mod error;
mod id;
mod origin;
pub mod percent;
mod units;
mod weburl;

pub use error::IdError;
pub use id::{is_id, object_segment};
pub use origin::{Origin, UrlScheme, split_authority};
pub use units::{Bytes, Count, Dims, MicroUsd, Permille, Px, Tokens, UnixSeconds};
pub use weburl::WebUrl;
