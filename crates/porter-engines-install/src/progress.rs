//! How far an install has come, and the way to stop it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::Notify;

#[derive(Debug, Default)]
struct Cells {
    done: AtomicU64,
    total: AtomicU64,
}

/// Bytes done of bytes expected, shared between the install that counts them and whoever shows
/// them.
#[derive(Debug, Clone, Default)]
pub struct Meter(Arc<Cells>);

impl Meter {
    /// A meter expecting `total` bytes.
    pub fn new(total: u64) -> Self {
        let meter = Self::default();
        meter.0.total.store(total, Ordering::Relaxed);
        meter
    }

    /// `bytes` more are done.
    pub fn add(&self, bytes: u64) {
        self.0.done.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Exactly `bytes` are done (a resumed download starts from what it already holds).
    pub fn set_done(&self, bytes: u64) {
        self.0.done.store(bytes, Ordering::Relaxed);
    }

    /// Bytes done.
    pub fn done(&self) -> u64 {
        self.0.done.load(Ordering::Relaxed)
    }

    /// Bytes expected.
    pub fn total(&self) -> u64 {
        self.0.total.load(Ordering::Relaxed)
    }

    /// Thousandths done, never past 1000; 0 while nothing is expected.
    pub fn permille(&self) -> u32 {
        let (done, total) = (u128::from(self.done()), u128::from(self.total()));
        if total == 0 {
            return 0;
        }
        u32::try_from((done * 1000 / total).min(1000)).unwrap_or(1000)
    }
}

#[derive(Debug, Default)]
struct Flag {
    stopped: AtomicBool,
    wake: Notify,
}

/// A request to stop, which an install looks at between pieces and which wakes a wait.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<Flag>);

impl CancelToken {
    /// A token nobody has used.
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks everything holding this token to stop.
    pub fn cancel(&self) {
        self.0.stopped.store(true, Ordering::SeqCst);
        self.0.wake.notify_waiters();
    }

    /// Whether a stop was asked for.
    pub fn is_cancelled(&self) -> bool {
        self.0.stopped.load(Ordering::SeqCst)
    }

    /// Resolves once a stop is asked for.
    pub async fn cancelled(&self) {
        loop {
            // A `Notified` made before the flag is read is woken by a `cancel` after it.
            let waiting = self.0.wake.notified();
            if self.is_cancelled() {
                return;
            }
            waiting.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meter_counts_in_thousandths_and_never_passes_a_whole() {
        let meter = Meter::new(200);
        assert_eq!(meter.permille(), 0);
        meter.add(50);
        assert_eq!(meter.permille(), 250);
        meter.add(500);
        assert_eq!(meter.permille(), 1000);
        meter.set_done(100);
        assert_eq!((meter.done(), meter.permille()), (100, 500));
        assert_eq!(Meter::default().permille(), 0);
    }

    #[tokio::test]
    async fn a_waiter_wakes_when_the_token_is_cancelled_and_a_late_one_does_not_wait() {
        let token = CancelToken::new();
        let waiter = tokio::spawn({
            let token = token.clone();
            async move { token.cancelled().await }
        });
        tokio::task::yield_now().await;
        assert!(!token.is_cancelled());
        token.cancel();
        waiter.await.expect("the waiter ended");
        token.cancelled().await;
    }
}
