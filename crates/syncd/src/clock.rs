//! What time it is, as a seam: the engine dates tombstones, anchors and conflicts by it.
//!
//! `Clock` and `SystemClock` moved to `porter_core::clock` (lane layers-clock); these paths stay
//! for existing imports. `ManualClock` stays here: a test shares it through an `Arc` and sets it
//! with `&self`, which porter_core's `FixedClock` (set with `&mut self`) cannot do.

/// A source of the current time, moved to `porter_core::clock::Clock`.
/// The system clock, moved to `porter_core::clock::SystemClock`.
pub use porter_core::clock::{Clock, SystemClock};

/// A clock a test sets.
#[derive(Debug, Default)]
pub struct ManualClock(std::sync::atomic::AtomicI64);

impl ManualClock {
    /// A clock at `at`.
    pub fn at(at: i64) -> Self {
        Self(std::sync::atomic::AtomicI64::new(at))
    }

    /// Moves it to `at`.
    pub fn set(&self, at: i64) {
        self.0.store(at, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> porter_core::UnixSeconds {
        porter_core::UnixSeconds(self.0.load(std::sync::atomic::Ordering::Relaxed))
    }
}
