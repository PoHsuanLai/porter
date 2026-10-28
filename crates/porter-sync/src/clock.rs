//! The replica's clock: it dates the tombstones a replica reports when the server keeps no
//! deletion time of its own. A replica holds and clones one, so it is a value over the shared
//! `porter_ids::clock::Clock` seam; whoever builds the replica says which clock it reads.

use porter_ids::UnixSeconds;
use porter_ids::{clock::Clock as CoreClock, clock::FixedClock};
use std::fmt;
use std::sync::Arc;

/// Reads the time for a replica.
#[derive(Clone)]
pub struct Clock(Arc<dyn CoreClock>);

impl Clock {
    /// A clock reading `clock`.
    pub fn new(clock: impl CoreClock + 'static) -> Self {
        Self(Arc::new(clock))
    }

    /// A clock that always says `at`.
    pub fn fixed(at: UnixSeconds) -> Self {
        Self::new(FixedClock(at))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_reads_the_clock_it_was_built_over() {
        let clock = Clock::fixed(UnixSeconds(42));
        assert_eq!(clock.clone().now(), UnixSeconds(42));
        assert_eq!(Clock::new(FixedClock(UnixSeconds(7))).now(), UnixSeconds(7));
    }
}
