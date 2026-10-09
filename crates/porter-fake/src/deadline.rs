//! The one deadline tests wait with. A wait that counts iterations of a sleep takes as long as
//! the machine lets it: a loop of 3000 sleeps of 20 ms is a minute on an idle computer and
//! several minutes on a starved one, whose timers are late. A [`Deadline`] is the clock's: it
//! ends when the time has passed, and says what was waited for and for how long.

use std::time::{Duration, Instant};

/// How long a test waits for something the daemon has to do. The daemons work slowly, never
/// wrongly, on a loaded machine (load average over a hundred, swap full: a connection made in
/// 20 ms idle took seconds); a test that waits for less than this would fail the daemon for the
/// machine's sake. A wait whose condition holds returns at once, so this costs only a failing
/// test.
pub const GENEROUS: Duration = Duration::from_secs(120);

/// A point in time a wait ends at.
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    began: Instant,
    limit: Duration,
}

impl Deadline {
    /// A deadline [`GENEROUS`] from now.
    pub fn generous() -> Self {
        Self::after(GENEROUS)
    }

    /// A deadline `limit` from now.
    pub fn after(limit: Duration) -> Self {
        Self {
            began: Instant::now(),
            limit,
        }
    }

    /// Whether the time is up.
    pub fn passed(&self) -> bool {
        self.began.elapsed() >= self.limit
    }

    /// How long has been waited so far.
    pub fn waited(&self) -> Duration {
        self.began.elapsed()
    }

    /// Fails the test: nothing named `what` happened in time.
    ///
    /// # Panics
    /// Always, naming `what` and the seconds waited.
    pub fn fail(&self, what: &str) -> ! {
        panic!(
            "never happened: {what} (waited {:.1} s of {} s)",
            self.waited().as_secs_f64(),
            self.limit.as_secs()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deadline_ends_by_the_clock_and_names_what_was_waited_for() {
        let soon = Deadline::after(Duration::from_millis(30));
        assert!(!soon.passed());
        std::thread::sleep(Duration::from_millis(60));
        assert!(soon.passed());
        let said = std::panic::catch_unwind(|| soon.fail("the daemon answers"))
            .expect_err("it panics")
            .downcast::<String>()
            .expect("a message");
        assert!(
            said.contains("never happened: the daemon answers"),
            "{said}"
        );
        assert!(said.contains("waited"), "{said}");
    }
}
