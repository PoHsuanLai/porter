//! The replica's clock: it dates the tombstones it reports, since a WebDAV server does not.

use porter_core::UnixSeconds;
use std::fmt;
use std::sync::Arc;

/// Reads the time.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> UnixSeconds + Send + Sync>);

impl Clock {
    /// A clock that always says `at`.
    pub fn fixed(at: UnixSeconds) -> Self {
        Self(Arc::new(move || at))
    }

    /// The system clock.
    pub fn system() -> Self {
        Self(Arc::new(|| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_secs());
            UnixSeconds(i64::try_from(now).unwrap_or(i64::MAX))
        }))
    }

    /// Now.
    pub fn now(&self) -> UnixSeconds {
        (self.0)()
    }
}

impl fmt::Debug for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Clock")
    }
}
