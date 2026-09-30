//! The system clock: the daemon edge is the one place porter reads the time.

use porter_core::UnixSeconds;
use porter_service::Clock;
use std::time::{SystemTime, UNIX_EPOCH};

/// The system's wall clock.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixSeconds {
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        UnixSeconds(i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
    }
}
