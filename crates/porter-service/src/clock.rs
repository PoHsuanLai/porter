//! The one clock seam: grants and audit entries are dated through it.

use porter_core::UnixSeconds;

/// The time now. accountd reads the system clock; tests pass a fixed one.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> UnixSeconds;
}
