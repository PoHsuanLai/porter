//! What time it is, as a seam: the engine dates tombstones, anchors and conflicts by it.
//!
//! `Clock` and `SystemClock` moved to `porter_core::clock` (lane layers-clock); these paths stay
//! for existing imports. `ManualClock` stays here: a test shares it through an `Arc` and sets it
//! with `&self`, which porter_core's `FixedClock` (set with `&mut self`) cannot do.

/// A source of the current time, moved to `porter_core::clock::Clock`.
/// The system clock, moved to `porter_core::clock::SystemClock`.
pub use porter_core::clock::{Clock, SystemClock};

/// The system clock, running `scale` times faster than real time (see
/// `scheduler::Settings::time_scale`); with a scale of 1 it is exactly [`SystemClock`]. Every
/// `Wall` in a process counts from the same start, so the engines of one test agree on the time.
#[derive(Debug, Clone, Copy)]
pub struct Wall {
    scale: u32,
}

impl Wall {
    /// The clock of a scheduler that runs `scale` seconds to the real second.
    pub fn scaled(scale: u32) -> Self {
        Self {
            scale: scale.max(1),
        }
    }
}

impl Clock for Wall {
    fn now(&self) -> porter_core::UnixSeconds {
        if self.scale == 1 {
            return SystemClock.now();
        }
        static START: std::sync::OnceLock<(std::time::Instant, porter_core::UnixSeconds)> =
            std::sync::OnceLock::new();
        let (began, at) = START.get_or_init(|| (std::time::Instant::now(), SystemClock.now()));
        let passed = began.elapsed().as_millis() * u128::from(self.scale) / 1000;
        porter_core::UnixSeconds(at.0.saturating_add(i64::try_from(passed).unwrap_or(i64::MAX)))
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
    fn now(&self) -> porter_core::UnixSeconds {
        porter_core::UnixSeconds(self.0.load(std::sync::atomic::Ordering::Relaxed))
    }
}
