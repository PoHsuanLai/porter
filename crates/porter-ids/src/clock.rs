//! The time, as a seam: the one `Clock` trait every crate that dates something takes, the system's
//! wall clock, and a fixed clock for tests. Nothing here reads the time unless a caller asks
//! [`SystemClock`] for it, and the daemons are the callers.

use crate::UnixSeconds;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The time now. Grants, audit entries, tombstones, sessions and anchors are dated through it;
/// tests pass a fixed one.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> UnixSeconds;
}

/// The system's wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixSeconds {
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        UnixSeconds(i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
    }
}

/// A clock that always says the same time. A test sets it again with [`FixedClock::set`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedClock(pub UnixSeconds);

impl FixedClock {
    /// A clock at `at`.
    pub const fn new(at: UnixSeconds) -> Self {
        Self(at)
    }

    /// Moves it to `at`.
    pub fn set(&mut self, at: UnixSeconds) {
        self.0 = at;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> UnixSeconds {
        self.0
    }
}

impl<T: Clock + ?Sized> Clock for Arc<T> {
    fn now(&self) -> UnixSeconds {
        (**self).now()
    }
}

/// A monotonic reading, for what is paced rather than dated (a rate limit's window). It is a seam
/// beside [`Clock`] so that a test moves it by hand.
pub trait Monotonic: Send + Sync {
    /// Now, on a clock that never goes back.
    fn now(&self) -> Instant;
}

/// The system's monotonic clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemMonotonic;

impl Monotonic for SystemMonotonic {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A monotonic clock that stands still until a test moves it with [`ManualMonotonic::advance`].
#[derive(Debug)]
pub struct ManualMonotonic {
    start: Instant,
    moved: std::sync::Mutex<std::time::Duration>,
}

impl ManualMonotonic {
    /// A clock standing at the moment it was made.
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            moved: std::sync::Mutex::default(),
        }
    }

    /// Moves it on by `by`.
    pub fn advance(&self, by: std::time::Duration) {
        *self.moved.lock().unwrap_or_else(|p| p.into_inner()) += by;
    }
}

impl Default for ManualMonotonic {
    fn default() -> Self {
        Self::new()
    }
}

impl Monotonic for ManualMonotonic {
    fn now(&self) -> Instant {
        self.start + *self.moved.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl<T: Monotonic + ?Sized> Monotonic for Arc<T> {
    fn now(&self) -> Instant {
        (**self).now()
    }
}
