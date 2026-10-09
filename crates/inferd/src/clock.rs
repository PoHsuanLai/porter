//! The wall clock, injected: the one place inferd reads it.
//!
//! The clock moved to `porter_core::clock` (lane layers-clock); these paths stay for existing
//! imports.

/// Seconds since the epoch, moved to `porter_core::clock::Clock`.
/// The system clock, moved to `porter_core::clock::SystemClock`.
/// A clock that always says the same, moved to `porter_core::clock::FixedClock`.
pub use porter_core::clock::{Clock, FixedClock, SystemClock};
