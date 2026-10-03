//! The wall clock, injected: the one place inferd reads it.

use porter_core::UnixSeconds;
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the epoch.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> UnixSeconds;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixSeconds {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        UnixSeconds(i64::try_from(seconds).unwrap_or(i64::MAX))
    }
}

/// A clock that always says the same.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub UnixSeconds);

impl Clock for FixedClock {
    fn now(&self) -> UnixSeconds {
        self.0
    }
}
