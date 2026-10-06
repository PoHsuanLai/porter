//! The datasets syncd carries, each a [`crate::dataset::Dataset`] over some local store.
//!
//! - `pim`: calendars and address books mirrored from a CalDAV/CardDAV server into a local vdir
//!   (W6e), read-only.
//! - `photos`: originals by content and an HLC-stamped metadata manifest (W6f), off by default.

pub mod photos;
pub mod pim;
