//! The time, as a seam: the one `Clock` trait every crate that dates something takes, the system's
//! wall clock, and a fixed clock for tests. Nothing here reads the time unless a caller asks
//! [`SystemClock`] for it, and the daemons are the callers.

use crate::UnixSeconds;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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
