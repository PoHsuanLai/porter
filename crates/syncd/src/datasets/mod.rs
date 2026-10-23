//! The datasets syncd carries, each a [`crate::dataset::Dataset`] over some local store.
//!
//! - `pim`: calendars and address books mirrored from a CalDAV/CardDAV server into a local vdir
//!   (W6e), read-only.
//! - `storage`: the app folder of a Graph account mirrored two ways, and the supervisor that
//!   also runs `photos` for it.
//! - `photos`: originals by content and an HLC-stamped metadata manifest (W6f), off by default.
//! - `drive`: a Storage account's whole drive, listed only (no download), for Files.

pub mod drive;
pub mod photos;
pub mod pim;
pub mod storage;
