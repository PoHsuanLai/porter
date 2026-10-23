//! What of a [`Snapshot`] a reader can see, so a pump that changed nothing wakes nobody.

use super::Snapshot;

/// How a snapshot differs from the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    /// Nothing a reader sees: at most the clock moved.
    Unseen,
    /// An engine's state, a failure, the GPU, or a pause that has run out.
    Seen,
}

impl Snapshot {
    /// The change from `self` to `next`. The clock alone is none; a failure's pause ending is
    /// one (the engine reads as loadable again).
    pub(super) fn change_to(&self, next: &Snapshot) -> Change {
        let pause_ended = self
            .failures
            .values()
            .any(|failure| self.now < Some(failure.retry_at) && next.now >= Some(failure.retry_at));
        if pause_ended
            || self.states != next.states
            || self.failures != next.failures
            || self.gpu != next.gpu
            || self.released != next.released
        {
            Change::Seen
        } else {
            Change::Unseen
        }
    }
}
