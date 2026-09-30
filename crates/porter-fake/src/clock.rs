//! A clock that always says the same time.

use porter_core::UnixSeconds;
use porter_service::Clock;

/// A fixed instant.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub UnixSeconds);

impl Clock for FixedClock {
    fn now(&self) -> UnixSeconds {
        self.0
    }
}
