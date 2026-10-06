//! What time it is, as a seam: the engine dates tombstones, anchors and conflicts by it.

use porter_core::UnixSeconds;

/// A source of the current time.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> UnixSeconds;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixSeconds {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        UnixSeconds(i64::try_from(seconds).unwrap_or(i64::MAX))
    }
}

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
    fn now(&self) -> UnixSeconds {
        UnixSeconds(self.0.load(std::sync::atomic::Ordering::Relaxed))
    }
}

impl<T: Clock + ?Sized> Clock for std::sync::Arc<T> {
    fn now(&self) -> UnixSeconds {
        (**self).now()
    }
}
