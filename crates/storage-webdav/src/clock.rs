//! The replica's clock: it dates the tombstones it reports, since a WebDAV server does not.
//!
//! A thin adapter over `porter_core::clock` (lane layers-clock): this type keeps its own shape
//! (a value its replicas hold and clone) and its constructors, and delegates to the shared clocks.

use porter_core::UnixSeconds;
use porter_core::clock::{Clock as CoreClock, FixedClock, SystemClock};
use std::fmt;
use std::sync::Arc;

/// Reads the time.
#[derive(Clone)]
pub struct Clock(Arc<dyn CoreClock>);

impl Clock {
    /// A clock that always says `at`.
    pub fn fixed(at: UnixSeconds) -> Self {
        Self(Arc::new(FixedClock(at)))
    }

    /// The system clock.
    pub fn system() -> Self {
        Self(Arc::new(SystemClock))
    }

    /// Now.
    pub fn now(&self) -> UnixSeconds {
        self.0.now()
    }
}

impl fmt::Debug for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Clock")
    }
}
