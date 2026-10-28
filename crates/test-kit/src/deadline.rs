//! The one deadline tests wait with. A wait that counts iterations of a sleep takes as long as
//! the machine lets it: a loop of 3000 sleeps of 20 ms is a minute on an idle computer and
//! several minutes on a starved one, whose timers are late. A [`Deadline`] is the clock's: it
//! ends when the time has passed, and says what was waited for and for how long.
//!
//! A wait for work that grows with the machine (a round of twenty large items) uses the idle
//! form: [`Deadline::idle`] ends only when no [`Deadline::progress`] has come for its limit, so a
//! slow machine that keeps working is never cut short, and a daemon that stops is still caught.

use std::time::{Duration, Instant};

/// How long a test waits for something the daemon has to do. The daemons work slowly, never
/// wrongly, on a loaded machine (load average over a hundred, swap full: a connection made in
/// 20 ms idle took seconds); a test that waits for less than this would fail the daemon for the
/// machine's sake. A wait whose condition holds returns at once, so this costs only a failing
/// test.
pub const GENEROUS: Duration = Duration::from_secs(120);

/// A point in time a wait ends at, or, for the idle form, a limit on how long a wait may go on
/// without progress.
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    began: Instant,
    progressed: Instant,
    limit: Duration,
}

impl Deadline {
    /// A deadline [`GENEROUS`] from now.
    pub fn generous() -> Self {
        Self::after(GENEROUS)
    }

    /// A deadline `limit` from now.
    pub fn after(limit: Duration) -> Self {
        let now = Instant::now();
        Self {
            began: now,
            progressed: now,
            limit,
        }
    }

    /// An idle deadline: it ends once `limit` has passed since the last [`Self::progress`], or
    /// since it began when no progress has come. Use it for a wait whose work grows with a loaded
    /// machine: each item done is progress, so only a daemon that stops makes the wait fail.
    pub fn idle(limit: Duration) -> Self {
        Self::after(limit)
    }

    /// Records progress: the idle clock starts again from now.
    pub fn progress(&mut self) {
        self.progressed = Instant::now();
    }

    /// Whether the time is up: `limit` has passed since the last progress (or since it began).
    pub fn passed(&self) -> bool {
        self.progressed.elapsed() >= self.limit
    }

    /// How long has been waited so far, progress or not.
    pub fn waited(&self) -> Duration {
        self.began.elapsed()
    }

    /// Fails the test: nothing named `what` happened in time.
    ///
    /// # Panics
    /// Always, naming `what`, the seconds waited and the seconds without progress.
    pub fn fail(&self, what: &str) -> ! {
        panic!(
            "never happened: {what} (waited {:.1} s; {:.1} s of that without progress, the limit {} s)",
            self.waited().as_secs_f64(),
            self.progressed.elapsed().as_secs_f64(),
            self.limit.as_secs()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deadline_passes_by_the_clock_and_names_what_was_waited_for() {
        // Both checks are about a limit that cannot pass or cannot fail on a loaded machine: no
        // sleep is timed against a limit.
        let never_yet = Deadline::after(GENEROUS);
        assert!(!never_yet.passed());
        let past = Deadline::after(Duration::ZERO);
        assert!(past.passed());
        let said = std::panic::catch_unwind(|| past.fail("the daemon answers"))
            .expect_err("it panics")
            .downcast::<String>()
            .expect("a message");
        assert!(
            said.contains("never happened: the daemon answers"),
            "{said}"
        );
        assert!(said.contains("waited"), "{said}");
    }

    #[test]
    fn an_idle_deadline_passes_only_without_progress_for_its_limit() {
        let mut idle = Deadline::idle(GENEROUS);
        assert!(!idle.passed());
        idle.progress();
        assert!(!idle.passed());
        // With no limit to wait for, an idle deadline is past at once, progress or none.
        let mut zero = Deadline::idle(Duration::ZERO);
        assert!(zero.passed());
        zero.progress();
        assert!(zero.passed());
    }
}
