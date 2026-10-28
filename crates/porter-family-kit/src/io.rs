//! What the password families reach the world through: the HTTP client and the clock that paces
//! a poll. Each is handed in as a shared, type-erased value so no family reaches a runtime itself.

use porter_http::{SharedHttp, SharedSleep, Sleep};
use std::time::Duration;

/// How a sign-in that waits on the person (Login Flow v2) polls: each wait is one `first` longer
/// than the last up to `step`, and the sign-in gives up once `limit` has been waited out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pacing {
    /// The first wait, and how much each later one grows by.
    pub first: Duration,
    /// The longest single wait.
    pub step: Duration,
    /// How long the person has in all.
    pub limit: Duration,
}

impl Default for Pacing {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(2),
            step: Duration::from_secs(10),
            limit: Duration::from_secs(20 * 60),
        }
    }
}

impl Pacing {
    /// The wait before poll number `attempt` (counting from 0).
    pub fn wait(&self, attempt: u32) -> Duration {
        self.first
            .saturating_mul(attempt.saturating_add(1))
            .min(self.step)
    }
}

/// The seams a password family dials through.
#[derive(Debug, Clone)]
pub struct Io {
    /// The HTTP client.
    pub http: SharedHttp,
    /// The clock that paces a poll.
    pub sleep: SharedSleep,
    /// How a sign-in polls.
    pub pacing: Pacing,
}

impl Io {
    /// The seams over `http` and `sleep`, with the default pacing.
    pub fn new(http: SharedHttp, sleep: impl Sleep + 'static) -> Self {
        Self {
            http,
            sleep: SharedSleep::new(sleep),
            pacing: Pacing::default(),
        }
    }
}
