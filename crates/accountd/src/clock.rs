//! The system clock: the daemon edge is the one place porter reads the time.
//!
//! `SystemClock` moved to `porter_core::clock` (lane layers-clock); this path stays for existing
//! imports.

/// The system's wall clock, moved to `porter_core::clock::SystemClock`.
pub(crate) use porter_core::clock::SystemClock;
