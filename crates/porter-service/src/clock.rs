//! The one clock seam: grants and audit entries are dated through it.
//!
//! The trait moved to `porter_core::clock` (lane layers-clock); this path stays for existing imports.

/// The time now, moved to `porter_core::clock::Clock`. accountd reads the system clock; tests pass
/// a fixed one.
pub use porter_core::clock::Clock;
